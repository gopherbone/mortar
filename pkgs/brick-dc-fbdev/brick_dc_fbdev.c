// SPDX-License-Identifier: GPL-2.0
/*
 * brick_dc_fbdev: PowerVR "display class" (DC) device for the TrimUI Brick
 * that scans out GPU-rendered window surfaces through the sun4i DRM fbdev
 * emulation (/dev/fb0), zero-copy.
 *
 * PROPOSAL / UNTESTED. Written from the DDK callback contract in
 * services/include/kerneldisplay.h (Dual MIT/GPL) and the behaviour of the
 * stock 4.9 dc_sunxi.ko (which is IMG's dc_fbdev sample: its strings are
 * "fbdev device detected does not support ypan/ywrap. Flipping disabled.",
 * "fb_pan_display failed" ...). No vendor code copied.
 *
 * How it works
 *  - The stock libpvrNULL_WSEGL implements EGL window surfaces on top of the
 *    DC bridge: PVRSRVDCSystemBufferAcquire / DCBufferAlloc for the swap
 *    chain, PVRSRVDCContextConfigureWithFence for each eglSwapBuffers
 *    ("DoPresentFlip"), or RGXQueueTransfer into the system buffer
 *    ("DoPresentBlit").
 *  - sun4i-drm's fbdev emulation (drm_fbdev_dma) gives one physically
 *    contiguous CMA GEM object of xres x yres_virtual. With
 *    CONFIG_DRM_FBDEV_OVERALLOC=300 (Mortar's kernel has this) yres_virtual is
 *    3 x 768 = 2304 lines, i.e. three 3 MiB scanout buffers.
 *  - This driver hands those three slices to services as DC buffers (by
 *    physical page address; the display engine has no IOMMU on this board and
 *    the GPU MMU maps the pages), and flips with fb_pan_display(), which the
 *    DRM fb helper turns into a blocking atomic plane update (vsync-paced).
 *
 * Caveats
 *  - fbcon draws into the same memory. Unbind it while a GL app runs
 *    (echo 0 > /sys/class/vtconsole/vtcon1/bind) or put the VT in KD_GRAPHICS.
 *  - Panning fails with -EBUSY while some process is DRM master on card0
 *    (that is the right behaviour: a KMS client owns the screen).
 *  - No implicit fencing in this DDK config (SUPPORT_DMA_FENCE undefined);
 *    services only calls ContextConfigure after the render's fence signalled,
 *    so the flip is safe.
 */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/fb.h>
#include <linux/console.h>
#include <linux/workqueue.h>
#include <linux/slab.h>
#include <linux/mutex.h>
#include <drm/drm_fb_helper.h>
#include <drm/drm_client.h>
#include <drm/drm_gem_dma_helper.h>
#include "kerneldisplay.h"

#define FORMAT_BGRA8 90 /* IMG_PIXFMT_B8G8R8A8_UNORM (see brick_dc_metadata.c) */
#define MAX_BUFS 3

struct bdc_device {
	struct fb_info *fb;
	phys_addr_t base;	/* physical address of yoffset 0 */
	void *vaddr;
	u32 w, h, stride, frame_size, nbufs;
	unsigned long used;	/* bitmap of handed-out slices */
	IMG_HANDLE srv;
	struct workqueue_struct *wq;
	struct mutex lock;
	IMG_HANDLE retire_pending; /* config currently on screen */
};

struct bdc_buffer {
	struct bdc_device *dev;
	u32 index;
	bool system;
};

struct bdc_flip {
	struct work_struct work;
	struct bdc_device *dev;
	struct bdc_buffer *buf; /* NULL: pipe count 0 (blank/flush) */
	IMG_HANDLE config;
};

static struct bdc_device bdc;
static bool trace = true;
module_param(trace, bool, 0644);
#define T(fmt, ...) do { if (trace) pr_info("brick_dc_fbdev: " fmt "\n", ##__VA_ARGS__); } while (0)

static void bdc_get_info(IMG_HANDLE h, DC_DISPLAY_INFO *o)
{
	memset(o, 0, sizeof(*o));
	strscpy(o->szDisplayName, "Brick sun4i fbdev DC", sizeof(o->szDisplayName));
	o->ui32MinDisplayPeriod = 0;
	o->ui32MaxDisplayPeriod = 1;
	o->ui32MaxPipes = 1;
	o->bUnlatchedSupported = IMG_FALSE;
}

static PVRSRV_ERROR bdc_panel_count(IMG_HANDLE h, IMG_UINT32 *n)
{
	*n = 1;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_panel_query(IMG_HANDLE h, IMG_UINT32 size, IMG_UINT32 *n, PVRSRV_PANEL_INFO *o)
{
	*n = 0;
	if (!size || !o)
		return PVRSRV_ERROR_INVALID_PARAMS;
	memset(o, 0, sizeof(*o));
	o->sSurfaceInfo.sDims.ui32Width = bdc.w;
	o->sSurfaceInfo.sDims.ui32Height = bdc.h;
	o->sSurfaceInfo.sFormat.ePixFormat = FORMAT_BGRA8;
	o->sSurfaceInfo.sFormat.eMemLayout = PVRSRV_SURFACE_MEMLAYOUT_STRIDED;
	o->ui32RefreshRate = 60;
	o->ui32XDpi = o->ui32YDpi = 160;
	*n = 1;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_format_query(IMG_HANDLE h, IMG_UINT32 n, PVRSRV_SURFACE_FORMAT *f, IMG_UINT32 *s)
{
	T("format_query n=%u fmt=%u layout=%u", n, n ? f[0].ePixFormat : 0, n ? f[0].eMemLayout : 0);
	u32 i;

	for (i = 0; i < n; i++)
		s[i] = f[i].ePixFormat == FORMAT_BGRA8 && f[i].eMemLayout == PVRSRV_SURFACE_MEMLAYOUT_STRIDED;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_dim_query(IMG_HANDLE h, IMG_UINT32 n, PVRSRV_SURFACE_DIMS *d, IMG_UINT32 *s)
{
	T("dim_query n=%u %ux%u", n, n ? d[0].ui32Width : 0, n ? d[0].ui32Height : 0);
	u32 i;

	for (i = 0; i < n; i++)
		s[i] = d[i].ui32Width == bdc.w && d[i].ui32Height == bdc.h;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_context_create(IMG_HANDLE h, IMG_HANDLE *ctx)
{
	T("context_create");
	*ctx = &bdc;
	return PVRSRV_OK;
}

static void bdc_context_destroy(IMG_HANDLE ctx)
{
	T("context_destroy");
	flush_workqueue(bdc.wq);
	mutex_lock(&bdc.lock);
	if (bdc.retire_pending) {
		DCDisplayConfigurationRetired(bdc.retire_pending);
		bdc.retire_pending = NULL;
	}
	mutex_unlock(&bdc.lock);
}

static int bdc_pan(u32 index)
{
	struct fb_var_screeninfo var = bdc.fb->var;
	int ret;

	var.xoffset = 0;
	var.yoffset = index * bdc.h;
	/* same lock order as the FBIOPAN_DISPLAY ioctl (fb_chrdev.c) */
	console_lock();
	lock_fb_info(bdc.fb);
	ret = fb_pan_display(bdc.fb, &var);
	unlock_fb_info(bdc.fb);
	console_unlock();
	return ret;
}

static void bdc_flip_work(struct work_struct *w)
{
	struct bdc_flip *f = container_of(w, struct bdc_flip, work);
	int ret = 0;

	if (f->buf)
		ret = bdc_pan(f->buf->index); /* blocks until the commit (vblank) */
	if (ret)
		pr_warn_ratelimited("brick_dc_fbdev: pan to %u failed: %d\n", f->buf->index, ret);

	mutex_lock(&bdc.lock);
	/* The previous configuration left the screen: retire it. */
	if (bdc.retire_pending)
		DCDisplayConfigurationRetired(bdc.retire_pending);
	bdc.retire_pending = f->buf ? f->config : NULL;
	mutex_unlock(&bdc.lock);
	if (!f->buf)
		DCDisplayConfigurationRetired(f->config);
	kfree(f);
}

static PVRSRV_ERROR bdc_configure_check(IMG_HANDLE ctx, IMG_UINT32 n, PVRSRV_SURFACE_CONFIG_INFO *s, IMG_HANDLE *b)
{
	T("configure_check pipes=%u", n);
	return n <= 1 ? PVRSRV_OK : PVRSRV_ERROR_DC_TOO_MANY_PIPES;
}

static void bdc_configure(IMG_HANDLE ctx, IMG_UINT32 n, PVRSRV_SURFACE_CONFIG_INFO *s,
			  IMG_HANDLE *b, IMG_UINT32 period, IMG_HANDLE config)
{
	T("configure pipes=%u buf=%d period=%u", n, n ? (int)((struct bdc_buffer *)b[0])->index : -1, period);
	struct bdc_flip *f = kzalloc(sizeof(*f), GFP_KERNEL);

	if (!f) {
		DCDisplayConfigurationRetired(config);
		return;
	}
	f->dev = &bdc;
	f->buf = n ? b[0] : NULL;
	f->config = config;
	INIT_WORK(&f->work, bdc_flip_work);
	queue_work(bdc.wq, &f->work);
}

static PVRSRV_ERROR bdc_buffer_alloc(IMG_HANDLE ctx, DC_BUFFER_CREATE_INFO *i, IMG_DEVMEM_LOG2ALIGN_T *log2page,
				     IMG_UINT32 *pages, IMG_UINT32 *stride, IMG_HANDLE *out)
{
	T("buffer_alloc %ux%u fmt=%u layout=%u bpp=%u", i->sSurface.sDims.ui32Width, i->sSurface.sDims.ui32Height, i->sSurface.sFormat.ePixFormat, i->sSurface.sFormat.eMemLayout, i->ui32BPP);
	struct bdc_buffer *buf;
	u32 idx;

	if (i->sSurface.sDims.ui32Width != bdc.w || i->sSurface.sDims.ui32Height != bdc.h ||
	    i->sSurface.sFormat.ePixFormat != FORMAT_BGRA8) {
		pr_info("brick_dc_fbdev: reject alloc %ux%u fmt %u bpp %u\n", i->sSurface.sDims.ui32Width,
			i->sSurface.sDims.ui32Height, i->sSurface.sFormat.ePixFormat, i->ui32BPP);
		return PVRSRV_ERROR_UNSUPPORTED_PIXEL_FORMAT;
	}
	mutex_lock(&bdc.lock);
	/* slice 0 is the system buffer (what fbcon shows); swap chain gets 1..n-1 first */
	for (idx = 1; idx < bdc.nbufs + 1; idx++)
		if (!test_bit(idx % bdc.nbufs, &bdc.used))
			break;
	if (idx == bdc.nbufs + 1) {
		mutex_unlock(&bdc.lock);
		return PVRSRV_ERROR_OUT_OF_MEMORY;
	}
	idx %= bdc.nbufs;
	set_bit(idx, &bdc.used);
	mutex_unlock(&bdc.lock);
	buf = kzalloc(sizeof(*buf), GFP_KERNEL);
	if (!buf) {
		clear_bit(idx, &bdc.used);
		return PVRSRV_ERROR_OUT_OF_MEMORY;
	}
	buf->dev = &bdc;
	buf->index = idx;
	*log2page = PAGE_SHIFT;
	*pages = bdc.frame_size >> PAGE_SHIFT;
	*stride = bdc.stride;
	*out = buf;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_system_acquire(IMG_HANDLE h, IMG_DEVMEM_LOG2ALIGN_T *log2page, IMG_UINT32 *pages,
				       IMG_UINT32 *stride, IMG_HANDLE *out)
{
	T("system_acquire");
	struct bdc_buffer *buf = kzalloc(sizeof(*buf), GFP_KERNEL);

	if (!buf)
		return PVRSRV_ERROR_OUT_OF_MEMORY;
	buf->dev = &bdc;
	buf->index = 0;
	buf->system = true;
	*log2page = PAGE_SHIFT;
	*pages = bdc.frame_size >> PAGE_SHIFT;
	*stride = bdc.stride;
	*out = buf;
	return PVRSRV_OK;
}

static void bdc_system_release(IMG_HANDLE h)
{
	kfree(h);
}

static PVRSRV_ERROR bdc_buffer_acquire(IMG_HANDLE h, IMG_DEV_PHYADDR *pa, void **va)
{
	T("buffer_acquire idx=%u", ((struct bdc_buffer *)h)->index);
	struct bdc_buffer *buf = h;
	phys_addr_t base = bdc.base + (phys_addr_t)buf->index * bdc.frame_size;
	u32 i, n = bdc.frame_size >> PAGE_SHIFT;

	for (i = 0; i < n; i++)
		pa[i].uiAddr = base + ((phys_addr_t)i << PAGE_SHIFT);
	*va = (u8 *)bdc.vaddr + (size_t)buf->index * bdc.frame_size;
	return PVRSRV_OK;
}

static void bdc_buffer_release(IMG_HANDLE h)
{
}

static void bdc_buffer_free(IMG_HANDLE h)
{
	T("buffer_free idx=%u", ((struct bdc_buffer *)h)->index);
	struct bdc_buffer *buf = h;

	if (!buf->system)
		clear_bit(buf->index, &bdc.used);
	kfree(buf);
}

static DC_DEVICE_FUNCTIONS bdc_funcs = {
	.pfnGetInfo = bdc_get_info,
	.pfnPanelQueryCount = bdc_panel_count,
	.pfnPanelQuery = bdc_panel_query,
	.pfnFormatQuery = bdc_format_query,
	.pfnDimQuery = bdc_dim_query,
	.pfnContextCreate = bdc_context_create,
	.pfnContextDestroy = bdc_context_destroy,
	.pfnContextConfigure = bdc_configure,
	.pfnContextConfigureCheck = bdc_configure_check,
	.pfnBufferAlloc = bdc_buffer_alloc,
	.pfnBufferAcquire = bdc_buffer_acquire,
	.pfnBufferRelease = bdc_buffer_release,
	.pfnBufferFree = bdc_buffer_free,
	.pfnBufferSystemAcquire = bdc_system_acquire,
	.pfnBufferSystemRelease = bdc_system_release,
};

static int __init bdc_init(void)
{
	struct fb_info *fb;
	struct drm_fb_helper *helper;
	struct drm_gem_dma_object *dma;
	PVRSRV_ERROR e;

	struct device *de;
	struct drm_device *drm;

	/* registered_fb[] is private since 6.0: go via the sun4i DRM device. */
	de = bus_find_device_by_name(&platform_bus_type, NULL, "display-engine");
	if (!de)
		return -EPROBE_DEFER;
	drm = dev_get_drvdata(de);
	put_device(de); /* built-in sun4i-drm never unbinds on the Brick */
	if (!drm || !drm->fb_helper || !drm->fb_helper->info) {
		pr_err("brick_dc_fbdev: no sun4i fbdev emulation\n");
		return -ENODEV;
	}
	helper = drm->fb_helper;
	fb = helper->info;
	if (!helper->buffer || !helper->buffer->gem)
		return -ENODEV;
	dma = to_drm_gem_dma_obj(helper->buffer->gem);
	bdc.fb = fb;
	bdc.base = dma->dma_addr; /* no IOMMU in front of the DE: dma == phys */
	bdc.vaddr = fb->screen_buffer;
	bdc.w = fb->var.xres;
	bdc.h = fb->var.yres;
	bdc.stride = fb->fix.line_length;
	bdc.frame_size = PAGE_ALIGN(bdc.stride * bdc.h);
	if (bdc.frame_size != bdc.stride * bdc.h) {
		pr_err("brick_dc_fbdev: frame not page aligned\n");
		return -EINVAL;
	}
	bdc.nbufs = min_t(u32, fb->var.yres_virtual / bdc.h, MAX_BUFS);
	if (bdc.nbufs < 2 || !fb->fix.ypanstep) {
		pr_err("brick_dc_fbdev: need yres_virtual >= 2*yres and ypan (CONFIG_DRM_FBDEV_OVERALLOC>=200)\n");
		return -EINVAL;
	}
	mutex_init(&bdc.lock);
	bdc.wq = alloc_ordered_workqueue("brick_dc_fbdev", WQ_HIGHPRI);
	if (!bdc.wq)
		return -ENOMEM;
	e = DCRegisterDevice(&bdc_funcs, 2, &bdc, &bdc.srv);
	if (e != PVRSRV_OK) {
		destroy_workqueue(bdc.wq);
		pr_err("brick_dc_fbdev: DCRegisterDevice %d\n", e);
		return -ENODEV;
	}
	pr_info("brick_dc_fbdev: %ux%u stride %u, %u buffers at %pa\n", bdc.w, bdc.h, bdc.stride, bdc.nbufs, &bdc.base);
	return 0;
}

static void __exit bdc_exit(void)
{
	DCUnregisterDevice(bdc.srv);
	destroy_workqueue(bdc.wq);
}

module_init(bdc_init);
module_exit(bdc_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("PowerVR DC device scanning out through sun4i DRM fbdev emulation (Brick)");
MODULE_SOFTDEP("pre: pvrsrvkm");
