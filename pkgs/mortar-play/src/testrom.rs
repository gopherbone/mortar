// SPDX-License-Identifier: GPL-2.0-or-later
//! A tiny NES test program, assembled here so the tests need no third-party
//! ROM (`mortar-play make-test-rom out.nes`). Mapper 0 with battery-backed
//! PRG RAM. It sets a palette, fills the background with striped tiles,
//! plays a ~440 Hz square wave on pulse 1, scrolls one pixel per frame and
//! writes a frame counter to $6000 (save RAM). With A held the scroll stops
//! and the tone goes up an octave, so tests can see input arrive.

use std::collections::HashMap;

#[derive(Default)]
struct Asm {
    code: Vec<u8>,
    labels: HashMap<&'static str, u16>,
    rel: Vec<(usize, &'static str)>,
    abs: Vec<(usize, &'static str)>,
}

const ORG: u16 = 0xc000;

impl Asm {
    fn pc(&self) -> u16 {
        ORG + self.code.len() as u16
    }
    fn label(&mut self, l: &'static str) {
        self.labels.insert(l, self.pc());
    }
    fn b(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }
    /// opcode + 16-bit absolute address
    fn a(&mut self, op: u8, addr: u16) {
        self.b(&[op, addr as u8, (addr >> 8) as u8]);
    }
    /// opcode + label (absolute)
    fn al(&mut self, op: u8, l: &'static str) {
        self.code.push(op);
        self.abs.push((self.code.len(), l));
        self.b(&[0, 0]);
    }
    /// branch to label
    fn br(&mut self, op: u8, l: &'static str) {
        self.code.push(op);
        self.rel.push((self.code.len(), l));
        self.code.push(0);
    }
    fn finish(mut self) -> Vec<u8> {
        for (at, l) in &self.rel {
            let target = self.labels[l] as i32;
            let next = (ORG as usize + at + 1) as i32;
            let off = target - next;
            assert!((-128..=127).contains(&off), "branch to {l} out of range");
            self.code[*at] = off as i8 as u8;
        }
        for (at, l) in &self.abs {
            let t = self.labels[l];
            self.code[*at] = t as u8;
            self.code[at + 1] = (t >> 8) as u8;
        }
        self.code
    }
}

// opcodes used
const LDA_I: u8 = 0xa9;
const LDX_I: u8 = 0xa2;
const LDY_I: u8 = 0xa0;
const STA: u8 = 0x8d;
const STX: u8 = 0x8e;
const BIT: u8 = 0x2c;
const BPL: u8 = 0x10;
const BNE: u8 = 0xd0;
const BEQ: u8 = 0xf0;
const LDA_X: u8 = 0xbd;
const LDA_A: u8 = 0xad;
const AND_I: u8 = 0x29;
const CPX_I: u8 = 0xe0;
const JMP: u8 = 0x4c;

pub fn nes() -> Vec<u8> {
    let mut a = Asm::default();
    a.label("reset");
    a.b(&[0x78, 0xd8]); // sei, cld
    a.b(&[LDX_I, 0x40]);
    a.a(STX, 0x4017);
    a.b(&[LDX_I, 0xff, 0x9a, 0xe8]); // ldx #$ff, txs, inx
    a.a(STX, 0x2000);
    a.a(STX, 0x2001);
    a.a(STX, 0x4010);
    for l in ["vb1", "vb2"] {
        a.label(l);
        a.a(BIT, 0x2002);
        a.br(BPL, l);
    }
    // palette
    a.b(&[LDA_I, 0x3f]);
    a.a(STA, 0x2006);
    a.b(&[LDA_I, 0x00]);
    a.a(STA, 0x2006);
    a.b(&[LDX_I, 0x00]);
    a.label("pal");
    a.al(LDA_X, "palette");
    a.a(STA, 0x2007);
    a.b(&[0xe8, CPX_I, 0x20]); // inx, cpx #32
    a.br(BNE, "pal");
    // nametable 0 + attributes: tile = (x >> 2) & 3
    a.b(&[LDA_I, 0x20]);
    a.a(STA, 0x2006);
    a.b(&[LDA_I, 0x00]);
    a.a(STA, 0x2006);
    a.b(&[LDY_I, 0x04]);
    a.label("nt_outer");
    a.b(&[LDX_I, 0x00]);
    a.label("nt_inner");
    a.b(&[0x8a, 0x4a, 0x4a, AND_I, 0x03]); // txa, lsr, lsr, and #3
    a.a(STA, 0x2007);
    a.b(&[0xe8]); // inx
    a.br(BNE, "nt_inner");
    a.b(&[0x88]); // dey
    a.br(BNE, "nt_outer");
    // APU: pulse 1, 50% duty, constant volume 15, ~440 Hz (timer 253)
    a.b(&[LDA_I, 0x01]);
    a.a(STA, 0x4015);
    a.b(&[LDA_I, 0xbf]);
    a.a(STA, 0x4000);
    a.b(&[LDA_I, 0x08]);
    a.a(STA, 0x4001);
    a.b(&[LDA_I, 0xfd]);
    a.a(STA, 0x4002);
    a.b(&[LDA_I, 0x00]);
    a.a(STA, 0x4003);
    // scroll 0, background on (incl. leftmost column)
    a.a(STA, 0x2005);
    a.a(STA, 0x2005);
    a.b(&[LDA_I, 0x0a]);
    a.a(STA, 0x2001);

    a.label("main");
    a.a(BIT, 0x2002);
    a.br(BPL, "main");
    // read the pad: strobe, then the first bit is A
    a.b(&[LDA_I, 0x01]);
    a.a(STA, 0x4016);
    a.b(&[LDA_I, 0x00]);
    a.a(STA, 0x4016);
    a.a(LDA_A, 0x4016);
    a.b(&[AND_I, 0x01]);
    a.br(BEQ, "no_a");
    // A held: higher tone, no scroll
    a.b(&[LDA_I, 0x7e]);
    a.a(STA, 0x4002);
    a.al(JMP, "scroll");
    a.label("no_a");
    a.b(&[LDA_I, 0xfd]);
    a.a(STA, 0x4002);
    a.b(&[0xe6, 0x00]); // inc $00
    a.label("scroll");
    a.b(&[0xa5, 0x00]); // lda $00
    a.a(STA, 0x6000);
    a.a(STA, 0x2005);
    a.b(&[LDA_I, 0x00]);
    a.a(STA, 0x2005);
    a.al(JMP, "main");

    a.label("rti");
    a.b(&[0x40]);
    a.label("palette");
    a.b(&[
        0x21, 0x16, 0x2a, 0x12, 0x21, 0x27, 0x1a, 0x02, 0x21, 0x30, 0x14, 0x0c, 0x21, 0x38, 0x2c, 0x15, //
        0x21, 0x16, 0x2a, 0x12, 0x21, 0x27, 0x1a, 0x02, 0x21, 0x30, 0x14, 0x0c, 0x21, 0x38, 0x2c, 0x15,
    ]);
    let (reset, rti) = (a.labels["reset"], a.labels["rti"]);
    let code = a.finish();

    let mut prg = vec![0xffu8; 16384];
    prg[..code.len()].copy_from_slice(&code);
    // vectors at $fffa: NMI, RESET, IRQ
    for (i, v) in [rti, reset, rti].iter().enumerate() {
        prg[0x3ffa + i * 2] = *v as u8;
        prg[0x3ffa + i * 2 + 1] = (*v >> 8) as u8;
    }
    // CHR: tile 0 blank, 1 solid colour 1, 2 solid colour 2, 3 checker of 3/0
    let mut chr = vec![0u8; 8192];
    for row in 0..8 {
        chr[16 + row] = 0xff;
        chr[32 + 8 + row] = 0xff;
        let c = if row % 2 == 0 { 0xaa } else { 0x55 };
        chr[48 + row] = c;
        chr[48 + 8 + row] = c;
    }
    let mut rom = b"NES\x1a".to_vec();
    rom.extend_from_slice(&[1, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // 16K PRG, 8K CHR, battery
    rom.extend(prg);
    rom.extend(chr);
    rom
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout() {
        let r = super::nes();
        assert_eq!(r.len(), 16 + 16384 + 8192);
        assert_eq!(&r[..4], b"NES\x1a");
        // reset vector points at $c000
        assert_eq!((r[16 + 0x3ffc], r[16 + 0x3ffd]), (0x00, 0xc0));
        assert_eq!(r[16], 0x78);
    }
}
