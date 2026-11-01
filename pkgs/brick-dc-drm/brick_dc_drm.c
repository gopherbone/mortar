// SPDX-License-Identifier: GPL-2.0
/*
 * brick_dc_drm: PowerVR "display class" (DC) device for the TrimUI Brick.
 * It scans out GPU-rendered EGL window surfaces zero-copy, as an in-kernel
 * DRM client of the sun4i display engine.
 *
 * Written from the DDK callback contract in services/include/kerneldisplay.h
 * (Dual MIT/GPL). No vendor code copied.
 *
 * How it works
 *  - The stock libpvrNULL_WSEGL implements EGL window surfaces on top of the
 *    DC bridge: DCBufferAlloc for the swap chain, then one
 *    DCContextConfigure per eglSwapBuffers ("DoPresentFlip").
 *  - At load this driver opens a DRM client on sun4i-drm and allocates
 *    NBUFS full-screen XRGB8888 dumb buffers (GEM DMA, physically contiguous
 *    CMA; the display engine has no IOMMU, so dma_addr == phys). It hands
 *    them to services by physical page address; the GPU MMU maps them.
 *  - Each configure is queued to an ordered workqueue, which points the
 *    client's modeset at the buffer and does a blocking atomic commit
 *    (drm_client_modeset_commit). That returns once the new buffer has been
 *    latched at vblank (flip_done), so the previous configuration has left
 *    the screen and is retired then. Presentation is vsync-paced and does not
 *    tear.
 *  - When the swap chain goes away (configure with 0 pipes, or context
 *    destroy) the fbdev console's scanout is restored.
 *
 * Why not flip through /dev/fb0 (the earlier brick_dc_fbdev): the DRM fbdev
 * helper pans by a delta from fb_info.var.yoffset, which fbcon resets to 0 on
 * a console switch without moving the plane. After one GL run plus an fbcon
 * rebind, every later pan pointed past the end of the framebuffer, failed
 * with -ENOSPC, and the retire path ran unthrottled (700+ fps, the GPU
 * drawing into the buffer being scanned out). Owning the buffers also keeps
 * fbcon from drawing into them.
 *
 * Interplay with other DRM users
 *  - While a userspace process is DRM master (a KMS app), commits return
 *    -EBUSY. Flips are then skipped but still paced on vblank, and retired, so
 *    GL apps keep running without owning the screen.
 *  - No implicit fencing in this DDK config (SUPPORT_DMA_FENCE undefined);
 *    services only calls ContextConfigure after the render's fence
 *    signalled, so the flip is safe.
 */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/workqueue.h>
#include <linux/slab.h>
#include <linux/mutex.h>
#include <linux/delay.h>
#include <linux/debugfs.h>
#include <linux/fs.h>
#include <drm/drm_client.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_fb_helper.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_dma_helper.h>
#include <drm/drm_vblank.h>
#include "kerneldisplay.h"

#define FORMAT_BGRA8 90 /* IMG_PIXFMT_B8G8R8A8_UNORM == DRM XRGB8888/ARGB8888 bytes */
#define NBUFS 3

struct bdc_device {
	struct drm_device *drm;
	struct drm_client_dev client;
	struct drm_client_buffer *cbuf[NBUFS];
	phys_addr_t phys[NBUFS];
	void *vaddr[NBUFS];
	u32 w, h, stride, frame_size;
	IMG_HANDLE srv;
	struct workqueue_struct *wq;
	struct mutex lock;
	IMG_HANDLE retire_pending; /* configuration currently on screen */
	bool showing;		/* our buffers are (or were last) on the CRTC */
	u32 cur;		/* buffer of the last successful flip */
	struct dentry *debugfs;
	unsigned int flips, skipped;
};

struct bdc_context {
	u32 next;		/* next buffer index to hand out */
};

struct bdc_buffer {
	u32 index;
	bool system;
};

struct bdc_flip {
	struct work_struct work;
	struct bdc_buffer *buf; /* NULL: pipe count 0 (swap chain gone/blank) */
	IMG_HANDLE config;
};

static struct bdc_device bdc;
static bool trace;
module_param(trace, bool, 0644);
MODULE_PARM_DESC(trace, "Log every DC call (bring-up aid)");
#define T(fmt, ...) do { if (trace) pr_info("brick_dc_drm: " fmt "\n", ##__VA_ARGS__); } while (0)

static void bdc_get_info(IMG_HANDLE h, DC_DISPLAY_INFO *o)
{
	memset(o, 0, sizeof(*o));
	strscpy(o->szDisplayName, "Brick sun4i DRM DC", sizeof(o->szDisplayName));
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
	u32 i;

	T("format_query n=%u fmt=%u layout=%u", n, n ? f[0].ePixFormat : 0, n ? f[0].eMemLayout : 0);
	for (i = 0; i < n; i++)
		s[i] = f[i].ePixFormat == FORMAT_BGRA8 && f[i].eMemLayout == PVRSRV_SURFACE_MEMLAYOUT_STRIDED;
	return PVRSRV_OK;
}

static PVRSRV_ERROR bdc_dim_query(IMG_HANDLE h, IMG_UINT32 n, PVRSRV_SURFACE_DIMS *d, IMG_UINT32 *s)
{
	u32 i;

	T("dim_query n=%u %ux%u", n, n ? d[0].ui32Width : 0, n ? d[0].ui32Height : 0);
	for (i = 0; i < n; i++)
		s[i] = d[i].ui32Width == bdc.w && d[i].ui32Height == bdc.h;
	return PVRSRV_OK;
}

/* Put the fbdev console's framebuffer back on the CRTC. */
static void bdc_restore_console(void)
{
	struct drm_fb_helper *helper = bdc.drm->fb_helper;

	if (!bdc.showing)
		return;
	bdc.showing = false;
	if (helper)
		drm_fb_helper_restore_fbdev_mode_unlocked(helper, false);
}

static PVRSRV_ERROR bdc_context_create(IMG_HANDLE h, IMG_HANDLE *ctx)
{
	struct bdc_context *c = kzalloc(sizeof(*c), GFP_KERNEL);

	T("context_create");
	if (!c)
		return PVRSRV_ERROR_OUT_OF_MEMORY;
	c->next = 1; /* buffer 0 doubles as the system buffer: start at 1 */
	*ctx = c;
	return PVRSRV_OK;
}

static void bdc_context_destroy(IMG_HANDLE ctx)
{
	T("context_destroy (%u flips, %u skipped)", bdc.flips, bdc.skipped);
	flush_workqueue(bdc.wq);
	mutex_lock(&bdc.lock);
	if (bdc.retire_pending) {
		DCDisplayConfigurationRetired(bdc.retire_pending);
		bdc.retire_pending = NULL;
	}
	bdc_restore_console();
	mutex_unlock(&bdc.lock);
	kfree(ctx);
}

/* No commit possible (DRM master held elsewhere): still pace on vblank. */
static void bdc_wait_vblank(void)
{
	struct drm_crtc *crtc = bdc.client.modesets[0].crtc;

	if (crtc && !drm_crtc_vblank_get(crtc)) {
		drm_crtc_wait_one_vblank(crtc);
		drm_crtc_vblank_put(crtc);
	} else {
		usleep_range(16000, 17000);
	}
}

/* Blocking atomic commit of buffer @index; returns after it latched. */
static int bdc_show(u32 index)
{
	struct drm_mode_set *ms;
	int ret;

	mutex_lock(&bdc.client.modeset_mutex);
	drm_client_for_each_modeset(ms, &bdc.client) {
		ms->fb = bdc.cbuf[index]->fb;
		ms->x = 0;
		ms->y = 0;
	}
	mutex_unlock(&bdc.client.modeset_mutex);
	ret = drm_client_modeset_commit(&bdc.client);
	if (ret == -EBUSY) {
		bdc.skipped++;
		bdc_wait_vblank();
		return 0;
	}
	if (!ret) {
		bdc.cur = index;
		bdc.flips++;
		bdc.showing = true;
	}
	return ret;
}

static void bdc_flip_work(struct work_struct *w)
{
	struct bdc_flip *f = container_of(w, struct bdc_flip, work);
	int ret;

	mutex_lock(&bdc.lock);
	if (f->buf) {
		ret = bdc_show(f->buf->index);
		if (ret) {
			pr_warn_ratelimited("brick_dc_drm: flip to %u failed: %d\n", f->buf->index, ret);
			bdc_wait_vblank(); /* never retire faster than the panel */
		}
	}
	/* The previous configuration has left the screen: retire it. */
	if (bdc.retire_pending)
		DCDisplayConfigurationRetired(bdc.retire_pending);
	bdc.retire_pending = f->buf ? f->config : NULL;
	if (!f->buf) {
		bdc_restore_console();
		DCDisplayConfigurationRetired(f->config);
	}
	mutex_unlock(&bdc.lock);
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
	struct bdc_flip *f;

	T("configure pipes=%u buf=%d period=%u", n, n ? (int)((struct bdc_buffer *)b[0])->index : -1, period);
	f = kzalloc(sizeof(*f), GFP_KERNEL);
	if (!f) {
		DCDisplayConfigurationRetired(config);
		return;
	}
	f->buf = n ? b[0] : NULL;
	f->config = config;
	INIT_WORK(&f->work, bdc_flip_work);
	queue_work(bdc.wq, &f->work);
}

static PVRSRV_ERROR bdc_buffer_alloc(IMG_HANDLE ctx, DC_BUFFER_CREATE_INFO *i, IMG_DEVMEM_LOG2ALIGN_T *log2page,
				     IMG_UINT32 *pages, IMG_UINT32 *stride, IMG_HANDLE *out)
{
	struct bdc_context *c = ctx;
	struct bdc_buffer *buf;
	u32 idx;

	T("buffer_alloc %ux%u fmt=%u layout=%u bpp=%u", i->sSurface.sDims.ui32Width, i->sSurface.sDims.ui32Height,
	  i->sSurface.sFormat.ePixFormat, i->sSurface.sFormat.eMemLayout, i->ui32BPP);
	if (i->sSurface.sDims.ui32Width != bdc.w || i->sSurface.sDims.ui32Height != bdc.h ||
	    i->sSurface.sFormat.ePixFormat != FORMAT_BGRA8) {
		pr_info("brick_dc_drm: reject alloc %ux%u fmt %u bpp %u\n", i->sSurface.sDims.ui32Width,
			i->sSurface.sDims.ui32Height, i->sSurface.sFormat.ePixFormat, i->ui32BPP);
		return PVRSRV_ERROR_UNSUPPORTED_PIXEL_FORMAT;
	}
	buf = kzalloc(sizeof(*buf), GFP_KERNEL);
	if (!buf)
		return PVRSRV_ERROR_OUT_OF_MEMORY;
	/*
	 * Buffers are not owned exclusively: a context gets distinct ones for
	 * its swap chain, round robin. Only one swap chain can be on screen at a
	 * time anyway, and services leaks the DC buffers (never calls
	 * BufferFree) of a GL process that is killed mid-run; exclusive
	 * ownership turned that leak into "no flip buffers" for every later app.
	 */
	mutex_lock(&bdc.lock);
	idx = c->next;
	c->next = (c->next + 1) % NBUFS;
	mutex_unlock(&bdc.lock);
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
	struct bdc_buffer *buf = kzalloc(sizeof(*buf), GFP_KERNEL);

	T("system_acquire");
	if (!buf)
		return PVRSRV_ERROR_OUT_OF_MEMORY;
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
	struct bdc_buffer *buf = h;
	u32 i, n = bdc.frame_size >> PAGE_SHIFT;

	T("buffer_acquire idx=%u", buf->index);
	for (i = 0; i < n; i++)
		pa[i].uiAddr = bdc.phys[buf->index] + ((phys_addr_t)i << PAGE_SHIFT);
	*va = bdc.vaddr[buf->index];
	return PVRSRV_OK;
}

static void bdc_buffer_release(IMG_HANDLE h)
{
}

static void bdc_buffer_free(IMG_HANDLE h)
{
	struct bdc_buffer *buf = h;

	T("buffer_free idx=%u", buf->index);
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

/*
 * debugfs "scanout": raw XRGB8888 contents (stride bytes per line) of the
 * buffer last flipped to the screen, for capturing what the panel shows.
 */
static ssize_t bdc_scanout_read(struct file *file, char __user *ubuf, size_t len, loff_t *pos)
{
	return simple_read_from_buffer(ubuf, len, pos, bdc.vaddr[READ_ONCE(bdc.cur)],
				       (size_t)bdc.stride * bdc.h);
}

static const struct file_operations bdc_scanout_fops = {
	.owner = THIS_MODULE,
	.read = bdc_scanout_read,
	.llseek = default_llseek,
};

static void bdc_free_buffers(void)
{
	int i;

	for (i = 0; i < NBUFS; i++) {
		drm_client_buffer_delete(bdc.cbuf[i]);
		bdc.cbuf[i] = NULL;
	}
}

static int bdc_setup_client(void)
{
	struct drm_mode_set *ms;
	int i, ret;

	ret = drm_client_init(bdc.drm, &bdc.client, "brick_dc_drm", NULL);
	if (ret)
		return ret;
	ret = drm_client_modeset_probe(&bdc.client, 0, 0);
	if (ret)
		goto err_release;
	bdc.w = 0;
	drm_client_for_each_modeset(ms, &bdc.client) {
		if (ms->mode && ms->num_connectors) {
			bdc.w = ms->mode->hdisplay;
			bdc.h = ms->mode->vdisplay;
			break;
		}
	}
	if (!bdc.w) {
		pr_err("brick_dc_drm: no connected display\n");
		ret = -ENODEV;
		goto err_release;
	}
	for (i = 0; i < NBUFS; i++) {
		struct drm_client_buffer *cb;
		struct drm_gem_dma_object *dma;

		cb = drm_client_buffer_create_dumb(&bdc.client, bdc.w, bdc.h, DRM_FORMAT_XRGB8888);
		if (IS_ERR(cb)) {
			ret = PTR_ERR(cb);
			goto err_free;
		}
		bdc.cbuf[i] = cb;
		dma = to_drm_gem_dma_obj(cb->fb->obj[0]);
		bdc.phys[i] = dma->dma_addr; /* no IOMMU in front of the DE: dma == phys */
		bdc.vaddr[i] = dma->vaddr;
		bdc.stride = cb->fb->pitches[0];
		if (!dma->vaddr || cb->fb->offsets[0] || dma->base.size < (size_t)bdc.stride * bdc.h) {
			ret = -EINVAL;
			goto err_free;
		}
		memset(dma->vaddr, 0, (size_t)bdc.stride * bdc.h);
	}
	bdc.frame_size = PAGE_ALIGN(bdc.stride * bdc.h);
	return 0;

err_free:
	bdc_free_buffers();
err_release:
	drm_client_release(&bdc.client);
	return ret;
}

static int __init bdc_init(void)
{
	struct device *de;
	PVRSRV_ERROR e;
	int ret;

	/* registered_fb[] is private since 6.0: go via the sun4i DRM device. */
	de = bus_find_device_by_name(&platform_bus_type, NULL, "display-engine");
	if (!de)
		return -EPROBE_DEFER;
	bdc.drm = dev_get_drvdata(de);
	put_device(de); /* built-in sun4i-drm never unbinds on the Brick */
	if (!bdc.drm)
		return -ENODEV;
	mutex_init(&bdc.lock);
	ret = bdc_setup_client();
	if (ret) {
		pr_err("brick_dc_drm: DRM client setup failed: %d\n", ret);
		return ret;
	}
	bdc.wq = alloc_ordered_workqueue("brick_dc_drm", WQ_HIGHPRI);
	if (!bdc.wq) {
		ret = -ENOMEM;
		goto err_client;
	}
	e = DCRegisterDevice(&bdc_funcs, 2, &bdc, &bdc.srv);
	if (e != PVRSRV_OK) {
		pr_err("brick_dc_drm: DCRegisterDevice %d\n", e);
		ret = -ENODEV;
		goto err_wq;
	}
	bdc.debugfs = debugfs_create_dir("brick_dc_drm", NULL);
	debugfs_create_file("scanout", 0400, bdc.debugfs, NULL, &bdc_scanout_fops);
	debugfs_create_u32("flips", 0400, bdc.debugfs, &bdc.flips);
	debugfs_create_u32("skipped", 0400, bdc.debugfs, &bdc.skipped);
	pr_info("brick_dc_drm: %ux%u stride %u, %u buffers at %pa %pa %pa\n", bdc.w, bdc.h, bdc.stride,
		NBUFS, &bdc.phys[0], &bdc.phys[1], &bdc.phys[2]);
	return 0;

err_wq:
	destroy_workqueue(bdc.wq);
err_client:
	bdc_free_buffers();
	drm_client_release(&bdc.client);
	return ret;
}

static void __exit bdc_exit(void)
{
	debugfs_remove_recursive(bdc.debugfs);
	DCUnregisterDevice(bdc.srv);
	destroy_workqueue(bdc.wq);
	mutex_lock(&bdc.lock);
	bdc_restore_console();
	mutex_unlock(&bdc.lock);
	bdc_free_buffers();
	drm_client_release(&bdc.client);
}

module_init(bdc_init);
module_exit(bdc_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("PowerVR DC device scanning out through a sun4i DRM client (Brick)");
MODULE_SOFTDEP("pre: pvrsrvkm");
