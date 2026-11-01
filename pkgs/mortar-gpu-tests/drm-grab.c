// SPDX-License-Identifier: GPL-2.0-or-later
// drm-grab: save what the panel shows as a PNG, whoever owns the display.
//
//   drm-grab [--card=/dev/dri/card0] [--plane=ID] OUT.png
//
// Takes the topmost enabled plane on the CRTC (or --plane), reads its
// framebuffer through GETFB2 + PRIME (root only: GETFB2 hands out handles
// to CAP_SYS_ADMIN) and writes it as 8-bit RGB. XRGB/ARGB8888 and
// XBGR/ABGR8888 only. Planes are not composited; the topmost one wins.
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <drm_fourcc.h>
#include <zlib.h>

static void put32(unsigned char *p, uint32_t v)
{
	p[0] = v >> 24; p[1] = v >> 16; p[2] = v >> 8; p[3] = v;
}

static void chunk(FILE *o, const char *type, const unsigned char *d, uint32_t n)
{
	unsigned char h[8];
	put32(h, n);
	memcpy(h + 4, type, 4);
	fwrite(h, 1, 8, o);
	if (n) fwrite(d, 1, n, o);
	uLong c = crc32(0, (const Bytef *)type, 4);
	c = crc32(c, d, n);
	put32(h, (uint32_t)c);
	fwrite(h, 1, 4, o);
}

int main(int argc, char **argv)
{
	const char *card = "/dev/dri/card0", *out = NULL;
	uint32_t want_plane = 0;
	for (int i = 1; i < argc; i++) {
		if (!strncmp(argv[i], "--card=", 7)) card = argv[i] + 7;
		else if (!strncmp(argv[i], "--plane=", 8)) want_plane = strtoul(argv[i] + 8, NULL, 0);
		else out = argv[i];
	}
	if (!out) {
		fprintf(stderr, "usage: drm-grab [--card=DEV] [--plane=ID] OUT.png\n");
		return 2;
	}
	int fd = open(card, O_RDWR | O_CLOEXEC);
	if (fd < 0) { perror(card); return 1; }
	drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1);
	drmModePlaneRes *pr = drmModeGetPlaneResources(fd);
	if (!pr) { perror("planes"); return 1; }

	uint32_t fb_id = 0, plane_id = 0;
	uint64_t best_z = 0;
	for (uint32_t i = 0; i < pr->count_planes; i++) {
		drmModePlane *p = drmModeGetPlane(fd, pr->planes[i]);
		if (!p) continue;
		if (p->fb_id && (!want_plane || p->plane_id == want_plane)) {
			uint64_t z = 0;
			drmModeObjectProperties *props = drmModeObjectGetProperties(fd, p->plane_id, DRM_MODE_OBJECT_PLANE);
			for (uint32_t k = 0; props && k < props->count_props; k++) {
				drmModePropertyRes *pp = drmModeGetProperty(fd, props->props[k]);
				if (pp && !strcmp(pp->name, "zpos")) z = props->prop_values[k];
				drmModeFreeProperty(pp);
			}
			drmModeFreeObjectProperties(props);
			if (!fb_id || z >= best_z) { fb_id = p->fb_id; plane_id = p->plane_id; best_z = z; }
		}
		drmModeFreePlane(p);
	}
	if (!fb_id) { fprintf(stderr, "no enabled plane\n"); return 1; }

	drmModeFB2 *fb = drmModeGetFB2(fd, fb_id);
	if (!fb || !fb->handles[0]) { fprintf(stderr, "GETFB2 %u: %s (need root)\n", fb_id, strerror(errno)); return 1; }
	int bgr;
	switch (fb->pixel_format) {
	case DRM_FORMAT_XRGB8888: case DRM_FORMAT_ARGB8888: bgr = 0; break;
	case DRM_FORMAT_XBGR8888: case DRM_FORMAT_ABGR8888: bgr = 1; break;
	default: fprintf(stderr, "format %.4s not supported\n", (char *)&fb->pixel_format); return 1;
	}
	int dfd;
	if (drmPrimeHandleToFD(fd, fb->handles[0], DRM_CLOEXEC, &dfd)) { perror("prime"); return 1; }
	size_t len = (size_t)fb->pitches[0] * fb->height + fb->offsets[0];
	unsigned char *m = mmap(NULL, len, PROT_READ, MAP_SHARED, dfd, 0);
	if (m == MAP_FAILED) { perror("mmap"); return 1; }

	uint32_t W = fb->width, H = fb->height;
	size_t rawn = (size_t)H * (1 + W * 3);
	unsigned char *raw = malloc(rawn), *row = malloc(fb->pitches[0]);
	for (uint32_t y = 0; y < H; y++) {
		unsigned char *d = raw + (size_t)y * (1 + W * 3);
		memcpy(row, m + fb->offsets[0] + (size_t)y * fb->pitches[0], W * 4); // one burst read of WC memory
		d[0] = 0;
		for (uint32_t x = 0; x < W; x++) {
			const unsigned char *s = row + x * 4;
			d[1 + x * 3 + 0] = bgr ? s[0] : s[2];
			d[1 + x * 3 + 1] = s[1];
			d[1 + x * 3 + 2] = bgr ? s[2] : s[0];
		}
	}
	uLongf zn = compressBound(rawn);
	unsigned char *z = malloc(zn);
	if (compress2(z, &zn, raw, rawn, 6) != Z_OK) { fprintf(stderr, "zlib\n"); return 1; }

	FILE *o = fopen(out, "wb");
	if (!o) { perror(out); return 1; }
	fwrite("\x89PNG\r\n\x1a\n", 1, 8, o);
	unsigned char ihdr[13];
	put32(ihdr, W); put32(ihdr + 4, H);
	ihdr[8] = 8; ihdr[9] = 2; ihdr[10] = ihdr[11] = ihdr[12] = 0;
	chunk(o, "IHDR", ihdr, 13);
	chunk(o, "IDAT", z, (uint32_t)zn);
	chunk(o, "IEND", NULL, 0);
	fclose(o);
	printf("plane %u fb %u %ux%u %.4s pitch %u -> %s\n", plane_id, fb_id, W, H,
	       (char *)&fb->pixel_format, fb->pitches[0], out);
	return 0;
}
