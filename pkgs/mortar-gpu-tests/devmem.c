// SPDX-License-Identifier: MIT
// devmem ADDR [COUNT] [WRITE_VALUE]: read COUNT 32-bit MMIO registers from
// ADDR (or write one value) via /dev/mem. For poking display/GPU registers.
// Only touch blocks whose clocks are on: reading a gated block hangs the bus.
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

int main(int argc, char **argv)
{
	if (argc < 2) { fprintf(stderr, "usage: devmem ADDR [COUNT] [WRITE_VALUE]\n"); return 2; }
	uint64_t addr = strtoull(argv[1], NULL, 0);
	unsigned count = argc > 2 ? strtoul(argv[2], NULL, 0) : 1;
	int fd = open("/dev/mem", O_RDWR | O_SYNC);
	if (fd < 0) { perror("/dev/mem"); return 1; }
	long pg = sysconf(_SC_PAGESIZE);
	uint64_t base = addr & ~(uint64_t)(pg - 1);
	size_t len = (addr - base) + count * 4;
	len = (len + pg - 1) & ~(size_t)(pg - 1);
	volatile uint32_t *m = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, base);
	if (m == MAP_FAILED) { perror("mmap"); return 1; }
	volatile uint32_t *r = (volatile uint32_t *)((volatile char *)m + (addr - base));
	if (argc > 3) { r[0] = strtoul(argv[3], NULL, 0); count = 1; }
	for (unsigned i = 0; i < count; i++)
		printf("%08llx: %08x\n", (unsigned long long)(addr + i * 4), r[i]);
	return 0;
}
