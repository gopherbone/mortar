// SPDX-License-Identifier: MIT
// sfxr.js port: sound seed -> parameters (exact, via the RC4 RNG) -> PCM.
//
// Web player pipeline being reproduced (cacheSeed + SoundEffect.generate + play):
//   synth runs at 44100 "ticks"/s with 8x supersampling, averaged over 8 ticks -> 5512.5Hz,
//   scaled by 1/8 * masterVolume(1) * (exp(SOUND_VOL=0.25)-1), each sample written 4x into a
//   22050Hz float AudioBuffer, played through three 1600Hz lowpass BiquadFilterNodes (Q=1).
// We output 22050Hz int16 (PS_SFX_REPEAT=4) and apply the same three biquads at that rate.
// Parameter generation is done in double exactly like JS; the per-sample synth uses float
// except for the few accumulators whose precision matters (frequency slide, hpf sweep).
#include "ps.h"
#include <math.h>

#ifdef __clang__
#pragma STDC FP_CONTRACT OFF
#endif

#ifndef PS_SFX_REPEAT
#define PS_SFX_REPEAT 4                 // output samples per 5512.5Hz sample (4 -> 22050Hz)
#endif
#define PS_SFX_RATE (44100 * PS_SFX_REPEAT / 8)

#define SOUND_VOL 0.25
#define SAMPLE_RATE 5512

enum { SQUARE = 0, SAWTOOTH = 1, SINE = 2, NOISE = 3, TRIANGLE = 4, BREAKER = 5, NSHAPES = 6 };

typedef struct {
    int wave_type;
    double p_env_attack, p_env_sustain, p_env_punch, p_env_decay;
    double p_base_freq, p_freq_limit, p_freq_ramp, p_freq_dramp;
    double p_vib_strength, p_vib_speed;
    double p_arp_mod, p_arp_speed;
    double p_duty, p_duty_ramp;
    double p_repeat_speed;
    double p_pha_offset, p_pha_ramp;
    double p_lpf_freq, p_lpf_ramp, p_lpf_resonance;
    double p_hpf_freq, p_hpf_ramp;
    double sound_vol;
    int sample_rate;
} Params;

// Params
static void params_init(Params *r)
{
    r->wave_type = SQUARE;
    r->p_env_attack = 0.0; r->p_env_sustain = 0.3; r->p_env_punch = 0.0; r->p_env_decay = 0.4;
    r->p_base_freq = 0.3; r->p_freq_limit = 0.0; r->p_freq_ramp = 0.0; r->p_freq_dramp = 0.0;
    r->p_vib_strength = 0.0; r->p_vib_speed = 0.0;
    r->p_arp_mod = 0.0; r->p_arp_speed = 0.0;
    r->p_duty = 0.0; r->p_duty_ramp = 0.0;
    r->p_repeat_speed = 0.0;
    r->p_pha_offset = 0.0; r->p_pha_ramp = 0.0;
    r->p_lpf_freq = 1.0; r->p_lpf_ramp = 0.0; r->p_lpf_resonance = 0.0;
    r->p_hpf_freq = 0.0; r->p_hpf_ramp = 0.0;
    r->sound_vol = 0.5;
    r->sample_rate = 44100;
}

// frnd (seeded branch)
static double frnd(PSRng *g, double range) { return ps_rng_uniform(g) * range; }
// rnd (seeded branch)
static int rnd(PSRng *g, int max) { return (int)floor(ps_rng_uniform(g) * (max + 1)); }
static int ifloor(double x) { return (int)floor(x); }

// pickupCoin
static void pickupCoin(PSRng *g, Params *r)
{
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = 0;
    r->p_base_freq = 0.4 + frnd(g, 0.5);
    r->p_env_attack = 0.0;
    r->p_env_sustain = frnd(g, 0.1);
    r->p_env_decay = 0.1 + frnd(g, 0.4);
    r->p_env_punch = 0.3 + frnd(g, 0.3);
    if (rnd(g, 1)) {
        r->p_arp_speed = 0.5 + frnd(g, 0.2);
        int num = ((int)frnd(g, 7) | 1) + 1;
        int den = num + ((int)frnd(g, 7) | 1) + 2;
        r->p_arp_mod = (double)num / (double)den;
    }
}

// laserShoot
static void laserShoot(PSRng *g, Params *r)
{
    r->wave_type = rnd(g, 2);
    if (r->wave_type == SINE && rnd(g, 1)) r->wave_type = rnd(g, 1);
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = SQUARE;
    r->p_base_freq = 0.5 + frnd(g, 0.5);
    r->p_freq_limit = r->p_base_freq - 0.2 - frnd(g, 0.6);
    if (r->p_freq_limit < 0.2) r->p_freq_limit = 0.2;
    r->p_freq_ramp = -0.15 - frnd(g, 0.2);
    if (rnd(g, 2) == 0) {
        r->p_base_freq = 0.3 + frnd(g, 0.6);
        r->p_freq_limit = frnd(g, 0.1);
        r->p_freq_ramp = -0.35 - frnd(g, 0.3);
    }
    if (rnd(g, 1)) {
        r->p_duty = frnd(g, 0.5);
        r->p_duty_ramp = frnd(g, 0.2);
    } else {
        r->p_duty = 0.4 + frnd(g, 0.5);
        r->p_duty_ramp = -frnd(g, 0.7);
    }
    r->p_env_attack = 0.0;
    r->p_env_sustain = 0.1 + frnd(g, 0.2);
    r->p_env_decay = frnd(g, 0.4);
    if (rnd(g, 1)) r->p_env_punch = frnd(g, 0.3);
    if (rnd(g, 2) == 0) {
        r->p_pha_offset = frnd(g, 0.2);
        r->p_pha_ramp = -frnd(g, 0.2);
    }
    if (rnd(g, 1)) r->p_hpf_freq = frnd(g, 0.3);
}

// explosion
static void explosion(PSRng *g, Params *r)
{
    if (rnd(g, 1)) {
        r->p_base_freq = 0.1 + frnd(g, 0.4);
        r->p_freq_ramp = -0.1 + frnd(g, 0.4);
    } else {
        r->p_base_freq = 0.2 + frnd(g, 0.7);
        r->p_freq_ramp = -0.2 - frnd(g, 0.2);
    }
    r->p_base_freq *= r->p_base_freq;
    if (rnd(g, 4) == 0) r->p_freq_ramp = 0.0;
    if (rnd(g, 2) == 0) r->p_repeat_speed = 0.3 + frnd(g, 0.5);
    r->p_env_attack = 0.0;
    r->p_env_sustain = 0.1 + frnd(g, 0.3);
    r->p_env_decay = frnd(g, 0.5);
    if (rnd(g, 1) == 0) {
        r->p_pha_offset = -0.3 + frnd(g, 0.9);
        r->p_pha_ramp = -frnd(g, 0.3);
    }
    r->p_env_punch = 0.2 + frnd(g, 0.6);
    if (rnd(g, 1)) {
        r->p_vib_strength = frnd(g, 0.7);
        r->p_vib_speed = frnd(g, 0.6);
    }
    if (rnd(g, 2) == 0) {
        r->p_arp_speed = 0.6 + frnd(g, 0.3);
        r->p_arp_mod = 0.8 - frnd(g, 1.6);
    }
}

// frnd(0.2) - 0.1 jitter used all over birdSound: (c + frnd(0.2)) - 0.1, left to right as in JS
#define J(c) ((c) + frnd(g, 0.2) - 0.1)

static void bird_wave(PSRng *g, Params *r)
{
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = SQUARE;
}

// birdSound
static void birdSound(PSRng *g, Params *r)
{
    if (frnd(g, 10) < 1) {
        bird_wave(g, r);
        r->p_env_attack = J(0.4304400932967592);
        r->p_env_sustain = J(0.15739346034252394);
        r->p_env_punch = J(0.004488201744871758);
        r->p_env_decay = J(0.07478075528212291);
        r->p_base_freq = J(0.9865265720147687);
        r->p_freq_limit = J(0);
        r->p_freq_ramp = J(-0.2995018224359539);
        if (frnd(g, 1.0) < 0.5) r->p_freq_ramp = 0.1 + frnd(g, 0.15);
        r->p_freq_dramp = 0.004598608156964473 + frnd(g, 0.1) - 0.05;
        r->p_vib_strength = J(-0.2202799497929496);
        r->p_vib_speed = J(0.8084998703158364);
        r->p_arp_mod = 0;
        r->p_arp_speed = 0;
        r->p_duty = J(-0.9031808754347107);
        r->p_duty_ramp = J(-0.8128699999808343);
        r->p_repeat_speed = J(0.6014860189319991);
        r->p_pha_offset = J(-0.9424902314367765);
        r->p_pha_ramp = J(-0.1055482222272056);
        r->p_lpf_freq = J(0.9989765717851521);
        r->p_lpf_ramp = J(-0.25051720626043017);
        r->p_lpf_resonance = J(0.32777871505494693);
        r->p_hpf_freq = J(0.0023548750981756753);
        r->p_hpf_ramp = J(-0.002375673204842568);
        return;
    }
    if (frnd(g, 10) < 1) {
        bird_wave(g, r);
        r->p_env_attack = J(0.5277795946672003);
        r->p_env_sustain = J(0.18243733568468432);
        r->p_env_punch = J(-0.020159754546840117);
        r->p_env_decay = J(0.1561353422051903);
        r->p_base_freq = J(0.9028855606533718);
        r->p_freq_limit = -0.008842787837148716;
        r->p_freq_ramp = -0.1;
        r->p_freq_dramp = -0.012891241489551925;
        r->p_vib_strength = J(-0.17923136138403065);
        r->p_vib_speed = J(0.908263385610142);
        r->p_arp_mod = J(0.41690153355414894);
        r->p_arp_speed = J(0.0010766233195860703);
        r->p_duty = J(-0.8735363011184684);
        r->p_duty_ramp = J(-0.7397985366747507);
        r->p_repeat_speed = J(0.0591789344172107);
        r->p_pha_offset = J(-0.9961184222777699);
        r->p_pha_ramp = J(-0.08234769395850523);
        r->p_lpf_freq = J(0.9412475115697335);
        r->p_lpf_ramp = J(-0.18261358925834958);
        r->p_lpf_resonance = J(0.24541438107389477);
        r->p_hpf_freq = J(-0.01831940280978611);
        r->p_hpf_ramp = J(-0.03857383633171346);
        return;
    }
    if (frnd(g, 10) < 1) {
        bird_wave(g, r);
        r->p_env_attack = J(0.4304400932967592);
        r->p_env_sustain = J(0.15739346034252394);
        r->p_env_punch = J(0.004488201744871758);
        r->p_env_decay = J(0.07478075528212291);
        r->p_base_freq = J(0.9865265720147687);
        r->p_freq_limit = J(0);
        r->p_freq_ramp = J(-0.2995018224359539);
        r->p_freq_dramp = J(0.004598608156964473);
        r->p_vib_strength = J(-0.2202799497929496);
        r->p_vib_speed = J(0.8084998703158364);
        r->p_arp_mod = J(-0.46410459213693644);
        r->p_arp_speed = J(-0.10955361249587248);
        r->p_duty = J(-0.9031808754347107);
        r->p_duty_ramp = J(-0.8128699999808343);
        r->p_repeat_speed = J(0.7014860189319991);
        r->p_pha_offset = J(-0.9424902314367765);
        r->p_pha_ramp = J(-0.1055482222272056);
        r->p_lpf_freq = J(0.9989765717851521);
        r->p_lpf_ramp = J(-0.25051720626043017);
        r->p_lpf_resonance = J(0.32777871505494693);
        r->p_hpf_freq = J(0.0023548750981756753);
        r->p_hpf_ramp = J(-0.002375673204842568);
        return;
    }
    if (frnd(g, 5) > 1) {
        bird_wave(g, r);
        if (rnd(g, 1)) {
            r->p_arp_mod = J(0.2697849293151393);
            r->p_arp_speed = J(-0.3131172257760948);
            r->p_base_freq = J(0.8090588299313949);
            r->p_duty = J(-0.6210022920964955);
            r->p_duty_ramp = J(-0.00043441813553182567);
            r->p_env_attack = J(0.004321877246874195);
            r->p_env_decay = J(0.1);
            r->p_env_punch = J(0.061737781504416146);
            r->p_env_sustain = J(0.4987252564798832);
            r->p_freq_dramp = J(0.31700340314222614);
            r->p_freq_limit = J(0);
            r->p_freq_ramp = J(-0.163380391341416);
            r->p_hpf_freq = J(0.4709005021145149);
            r->p_hpf_ramp = J(0.6924667290539194);
            r->p_lpf_freq = J(0.8351398631384511);
            r->p_lpf_ramp = J(0.36616557192873134);
            r->p_lpf_resonance = J(-0.08685777111664439);
            r->p_pha_offset = J(-0.036084571580025544);
            r->p_pha_ramp = J(-0.014806445085568108);
            r->p_repeat_speed = J(-0.8094368475518489);
            r->p_vib_speed = J(0.4496665457171294);
            r->p_vib_strength = J(0.23413762515532424);
        } else {
            r->p_arp_mod = J(-0.35697118026766184);
            r->p_arp_speed = J(0.3581140690559588);
            r->p_base_freq = J(1.3260897696157528);
            r->p_duty = J(-0.30984900436710694);
            r->p_duty_ramp = J(-0.0014374759133411626);
            r->p_env_attack = J(0.3160357835682254);
            r->p_env_decay = J(0.1);
            r->p_env_punch = J(0.24323114016870148);
            r->p_env_sustain = J(0.4);
            r->p_freq_dramp = J(0.2866475886237244);
            r->p_freq_limit = J(0);
            r->p_freq_ramp = J(-0.10956352368742976);
            r->p_hpf_freq = J(0.20772718017889846);
            r->p_hpf_ramp = J(0.1564090637378835);
            r->p_lpf_freq = J(0.6021372770637031);
            r->p_lpf_ramp = J(0.24016227139979027);
            r->p_lpf_resonance = J(-0.08787383821160144);
            r->p_pha_offset = J(-0.381597686151701);
            r->p_pha_ramp = J(-0.0002481687661373495);
            r->p_repeat_speed = J(0.07812112809425686);
            r->p_vib_speed = J(-0.13648848579133943);
            r->p_vib_strength = J(0.0018874158972302657);
        }
        return;
    }
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 1 || r->wave_type == 3) r->wave_type = 2;
    r->p_base_freq = 0.85 + frnd(g, 0.15);
    r->p_freq_ramp = 0.3 + frnd(g, 0.15);
    r->p_env_attack = 0 + frnd(g, 0.09);
    r->p_env_sustain = 0.2 + frnd(g, 0.3);
    r->p_env_decay = 0 + frnd(g, 0.1);
    r->p_duty = frnd(g, 2.0) - 1.0;
    r->p_duty_ramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_repeat_speed = 0.5 + frnd(g, 0.1);
    r->p_pha_offset = -0.3 + frnd(g, 0.9);
    r->p_pha_ramp = -frnd(g, 0.3);
    r->p_arp_speed = 0.4 + frnd(g, 0.6);
    r->p_arp_mod = 0.8 + frnd(g, 0.1);
    r->p_lpf_resonance = frnd(g, 2.0) - 1.0;
    r->p_lpf_freq = 1.0 - pow(frnd(g, 1.0), 3.0);
    r->p_lpf_ramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    if (r->p_lpf_freq < 0.1 && r->p_lpf_ramp < -0.05) r->p_lpf_ramp = -r->p_lpf_ramp;
    r->p_hpf_freq = pow(frnd(g, 1.0), 5.0);
    r->p_hpf_ramp = pow(frnd(g, 2.0) - 1.0, 5.0);
}
#undef J

// pushSound
static void pushSound(PSRng *g, Params *r)
{
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 2) r->wave_type++;
    if (r->wave_type == 0) r->wave_type = NOISE;
    r->p_base_freq = 0.1 + frnd(g, 0.4);
    r->p_freq_ramp = 0.05 + frnd(g, 0.2);
    r->p_env_attack = 0.01 + frnd(g, 0.09);
    r->p_env_sustain = 0.01 + frnd(g, 0.09);
    r->p_env_decay = 0.01 + frnd(g, 0.09);
    r->p_repeat_speed = 0.3 + frnd(g, 0.5);
    r->p_pha_offset = -0.3 + frnd(g, 0.9);
    r->p_pha_ramp = -frnd(g, 0.3);
    r->p_arp_speed = 0.6 + frnd(g, 0.3);
    r->p_arp_mod = 0.8 - frnd(g, 1.6);
}

// powerUp
static void powerUp(PSRng *g, Params *r)
{
    if (rnd(g, 1)) r->wave_type = SAWTOOTH;
    else r->p_duty = frnd(g, 0.6);
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = SQUARE;
    if (rnd(g, 1)) {
        r->p_base_freq = 0.2 + frnd(g, 0.3);
        r->p_freq_ramp = 0.1 + frnd(g, 0.4);
        r->p_repeat_speed = 0.4 + frnd(g, 0.4);
    } else {
        r->p_base_freq = 0.2 + frnd(g, 0.3);
        r->p_freq_ramp = 0.05 + frnd(g, 0.2);
        if (rnd(g, 1)) {
            r->p_vib_strength = frnd(g, 0.7);
            r->p_vib_speed = frnd(g, 0.6);
        }
    }
    r->p_env_attack = 0.0;
    r->p_env_sustain = frnd(g, 0.4);
    r->p_env_decay = 0.1 + frnd(g, 0.4);
}

// hitHurt
static void hitHurt(PSRng *g, Params *r)
{
    r->wave_type = rnd(g, 2);
    if (r->wave_type == SINE) r->wave_type = NOISE;
    if (r->wave_type == SQUARE) r->p_duty = frnd(g, 0.6);
    r->wave_type = ifloor(frnd(g, NSHAPES));
    r->p_base_freq = 0.2 + frnd(g, 0.6);
    r->p_freq_ramp = -0.3 - frnd(g, 0.4);
    r->p_env_attack = 0.0;
    r->p_env_sustain = frnd(g, 0.1);
    r->p_env_decay = 0.1 + frnd(g, 0.2);
    if (rnd(g, 1)) r->p_hpf_freq = frnd(g, 0.3);
}

// jump
static void jump(PSRng *g, Params *r)
{
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = SQUARE;
    r->p_duty = frnd(g, 0.6);
    r->p_base_freq = 0.3 + frnd(g, 0.3);
    r->p_freq_ramp = 0.1 + frnd(g, 0.2);
    r->p_env_attack = 0.0;
    r->p_env_sustain = 0.1 + frnd(g, 0.3);
    r->p_env_decay = 0.1 + frnd(g, 0.2);
    if (rnd(g, 1)) r->p_hpf_freq = frnd(g, 0.3);
    if (rnd(g, 1)) r->p_lpf_freq = 1.0 - frnd(g, 0.6);
}

// blipSelect
static void blipSelect(PSRng *g, Params *r)
{
    r->wave_type = rnd(g, 1);
    r->wave_type = ifloor(frnd(g, NSHAPES));
    if (r->wave_type == 3) r->wave_type = rnd(g, 1);
    if (r->wave_type == SQUARE) r->p_duty = frnd(g, 0.6);
    r->p_base_freq = 0.2 + frnd(g, 0.4);
    r->p_env_attack = 0.0;
    r->p_env_sustain = 0.1 + frnd(g, 0.1);
    r->p_env_decay = frnd(g, 0.2);
    r->p_hpf_freq = 0.1;
}

// random
static void randomSound(PSRng *g, Params *r)
{
    r->wave_type = ifloor(frnd(g, NSHAPES));
    r->p_base_freq = pow(frnd(g, 2.0) - 1.0, 2.0);
    if (rnd(g, 1)) r->p_base_freq = pow(frnd(g, 2.0) - 1.0, 3.0) + 0.5;
    r->p_freq_limit = 0.0;
    r->p_freq_ramp = pow(frnd(g, 2.0) - 1.0, 5.0);
    if (r->p_base_freq > 0.7 && r->p_freq_ramp > 0.2) r->p_freq_ramp = -r->p_freq_ramp;
    if (r->p_base_freq < 0.2 && r->p_freq_ramp < -0.05) r->p_freq_ramp = -r->p_freq_ramp;
    r->p_freq_dramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_duty = frnd(g, 2.0) - 1.0;
    r->p_duty_ramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_vib_strength = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_vib_speed = frnd(g, 2.0) - 1.0;
    r->p_env_attack = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_env_sustain = pow(frnd(g, 2.0) - 1.0, 2.0);
    r->p_env_decay = frnd(g, 2.0) - 1.0;
    r->p_env_punch = pow(frnd(g, 0.8), 2.0);
    if (r->p_env_attack + r->p_env_sustain + r->p_env_decay < 0.2) {
        r->p_env_sustain += 0.2 + frnd(g, 0.3);
        r->p_env_decay += 0.2 + frnd(g, 0.3);
    }
    r->p_lpf_resonance = frnd(g, 2.0) - 1.0;
    r->p_lpf_freq = 1.0 - pow(frnd(g, 1.0), 3.0);
    r->p_lpf_ramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    if (r->p_lpf_freq < 0.1 && r->p_lpf_ramp < -0.05) r->p_lpf_ramp = -r->p_lpf_ramp;
    r->p_hpf_freq = pow(frnd(g, 1.0), 5.0);
    r->p_hpf_ramp = pow(frnd(g, 2.0) - 1.0, 5.0);
    r->p_pha_offset = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_pha_ramp = pow(frnd(g, 2.0) - 1.0, 3.0);
    r->p_repeat_speed = frnd(g, 2.0) - 1.0;
    r->p_arp_speed = frnd(g, 2.0) - 1.0;
    r->p_arp_mod = frnd(g, 2.0) - 1.0;
}

typedef void (*Generator)(PSRng *, Params *);
static const Generator generators[10] = {
    pickupCoin, laserShoot, explosion, powerUp, hitHurt,
    jump, blipSelect, pushSound, randomSound, birdSound
};

// generateFromSeed(seed) with seed = the JS Number value. Returns false where JS would throw.
static bool generateFromSeed(double seed, Params *out)
{
    if (!(seed >= 0) || isinf(seed)) return false;
    // (seed / 100) | 0  -> ToInt32
    double q = trunc(seed / 100);
    double m = fmod(q, 4294967296.0);
    if (m >= 2147483648.0) m -= 4294967296.0;
    int32_t rs = (int32_t)m;
    // new RNG(number): JSON.stringify(number)
    char buf[16];
    int n = 0;
    uint32_t u = rs < 0 ? (uint32_t)0 - (uint32_t)rs : (uint32_t)rs;
    char tmp[12];
    int k = 0;
    do { tmp[k++] = (char)('0' + u % 10); u /= 10; } while (u);
    if (rs < 0) buf[n++] = '-';
    while (k) buf[n++] = tmp[--k];
    buf[n] = 0;
    PSRng g;
    ps_rng_init(&g, buf);
    int gi = (int)fmod(seed, 100);
    params_init(out);
    generators[gi % 10](&g, out);
    return true;
}

// sin(2*pi*x) for x in about [-0.5, 1.5]; |error| < 1e-6.
static float sin_turns(float x)
{
    float t = x - 0.5f;                     // sin(2pi x) = -sin(2pi t)
    if (t > 0.25f) t = 0.5f - t;
    else if (t < -0.25f) t = -0.5f - t;
    float u = t * 6.28318531f, u2 = u * u;
    float s = u * (1.0f + u2 * (-1.0f / 6 + u2 * (1.0f / 120 + u2 * (-1.0f / 5040 +
              u2 * (1.0f / 362880 + u2 * (-1.0f / 39916800))))));
    return -s;
}

typedef struct {        // per-repeat state (repeat() in SoundEffect.generate)
    int rep_time;
    double fperiod, fmaxperiod, fslide, fdslide;
    float square_duty, square_slide;
    double arp_mod;
    int arp_limit;
} RepState;

// SoundEffect.generate: repeat()
static void sfx_repeat(const Params *ps, RepState *st)
{
    st->rep_time = 0;
    st->fperiod = 100.0 / (ps->p_base_freq * ps->p_base_freq + 0.001);
    st->fmaxperiod = 100.0 / (ps->p_freq_limit * ps->p_freq_limit + 0.001);
    st->fslide = 1.0 - pow(ps->p_freq_ramp, 3.0) * 0.01;
    st->fdslide = -pow(ps->p_freq_dramp, 3.0) * 0.000001;
    st->square_duty = (float)(0.5 - ps->p_duty * 0.5);
    st->square_slide = (float)(-ps->p_duty_ramp * 0.00005);
    if (ps->p_arp_mod >= 0.0) st->arp_mod = 1.0 - pow(ps->p_arp_mod, 2.0) * 0.9;
    else st->arp_mod = 1.0 + pow(ps->p_arp_mod, 2.0) * 10.0;
    st->arp_limit = (int)floor(pow(1.0 - ps->p_arp_speed, 2.0) * 20000 + 32);
    if (ps->p_arp_speed == 1.0) st->arp_limit = 0;
}

typedef struct { float b0, b1, b2, a1, a2, x1, x2, y1, y2; } Biquad;

// WebAudio BiquadFilterNode "lowpass" (Q in dB, default 1).
static void biquad_lowpass(Biquad *f, double freq, double Q, double rate)
{
    double w0 = 2.0 * 3.14159265358979323846 * freq / rate;
    double alpha = sin(w0) / (2.0 * pow(10.0, Q / 20.0));
    double c = cos(w0), a0 = 1.0 + alpha;
    f->b0 = (float)((1.0 - c) / 2.0 / a0);
    f->b1 = (float)((1.0 - c) / a0);
    f->b2 = f->b0;
    f->a1 = (float)(-2.0 * c / a0);
    f->a2 = (float)((1.0 - alpha) / a0);
    f->x1 = f->x2 = f->y1 = f->y2 = 0;
}

static inline float biquad_run(Biquad *f, float x)
{
    float y = f->b0 * x + f->b1 * f->x1 + f->b2 * f->x2 - f->a1 * f->y1 - f->a2 * f->y2;
    f->x2 = f->x1; f->x1 = x;
    f->y2 = f->y1; f->y1 = y;
    return y;
}

static int emit(int16_t *out, int out_i, int out_len, float sample, Biquad *bq);

// stand-in for Math.random() in the noise buffer (JS is unseeded there anyway)
static float noise_rand(uint32_t *s)
{
    uint32_t x = *s;
    x ^= x << 13; x ^= x >> 17; x ^= x << 5;
    *s = x;
    return (float)(x >> 8) * (1.0f / 8388608.0f) - 1.0f;
}

// SoundEffect.generate + the play() filter chain
static int16_t *sfx_synth(const Params *ps, uint32_t noise_seed, int *nsamples)
{
    RepState st;
    sfx_repeat(ps, &st);

    // Filter
    float fltp = 0.0f, fltdp = 0.0f;
    const double fltw0 = pow(ps->p_lpf_freq, 3.0) * 0.1;
    const double fltw_d = 1.0 + ps->p_lpf_ramp * 0.0001;
    double fltdmp_d = 5.0 / (1.0 + pow(ps->p_lpf_resonance, 2.0) * 20.0) * (0.01 + fltw0);
    if (fltdmp_d > 0.8) fltdmp_d = 0.8;
    const float fltdmp = (float)fltdmp_d;
    float fltphp = 0.0f;
    // Multiplicative sweeps x *= d are done as x += x*(d-1) in float, so the tiny (d-1) isn't
    // rounded away the way a float d close to 1 would be.
    float flthp = (float)(pow(ps->p_hpf_freq, 2.0) * 0.1);
    const double flthp_d = 1.0 + ps->p_hpf_ramp * 0.0003;
    const float flthp_m = (float)(ps->p_hpf_ramp * 0.0003);
    float fltw = (float)fltw0;
    const float fltw_m = (float)(ps->p_lpf_ramp * 0.0001);
    const bool lpf_on = ps->p_lpf_freq != 1.0;
    float fwpow[9];     // fltw_d^k
    {
        double p = 1.0;
        for (int k = 0; k <= 8; k++) { fwpow[k] = (float)p; p *= fltw_d; }
    }
    const float fltw_m8 = (float)(pow(fltw_d, 8.0) - 1.0);

    // Vibrato (phase kept in [0, 2pi) as float)
    float vib_phase = 0.0f;
    const float vib_speed = (float)(pow(ps->p_vib_speed, 2.0) * 0.01);
    const float vib_amp = (float)(ps->p_vib_strength * 0.5);

    // Envelope
    int env_stage = 0, env_time = 0;
    int env_length[3] = {
        (int)floor(ps->p_env_attack * ps->p_env_attack * 100000.0),
        (int)floor(ps->p_env_sustain * ps->p_env_sustain * 100000.0),
        (int)floor(ps->p_env_decay * ps->p_env_decay * 100000.0)
    };
    int env_total_length = env_length[0] + env_length[1] + env_length[2];
    float env_inv[3];
    for (int k = 0; k < 3; k++) env_inv[k] = env_length[k] ? 1.0f / (float)env_length[k] : 0.0f;
    const float punch2 = (float)(2.0 * ps->p_env_punch);

    // Phaser: fphase advances linearly (never reset), so use the closed form
    double fphase0 = pow(ps->p_pha_offset, 2.0) * 1020.0;
    if (ps->p_pha_offset < 0.0) fphase0 = -fphase0;
    double fdphase0 = pow(ps->p_pha_ramp, 2.0) * 1.0;
    if (ps->p_pha_ramp < 0.0) fdphase0 = -fdphase0;
    const float fphase_f = (float)fphase0, fdphase_f = (float)fdphase0;
    int ipp = 0;
    int phase = 0;

    // Noise
    float noise_buffer[32];
    uint32_t nseed = noise_seed * 2654435761u + 0x12345u;
    if (!nseed) nseed = 1;
    for (int k = 0; k < 32; k++) noise_buffer[k] = noise_rand(&nseed);

    // Repeat
    int rep_limit = (int)floor(pow(1.0 - ps->p_repeat_speed, 2.0) * 20000 + 32);
    if (ps->p_repeat_speed == 0.0) rep_limit = 0;

    const double gain = exp(ps->sound_vol) - 1;
    const int summands = 44100 / ps->sample_rate;     // 8
    const int buffer_length = (env_total_length + summands - 1) / summands;
    const int out_len = PS_SFX_REPEAT * buffer_length;
    if (out_len <= 0) { *nsamples = 0; return NULL; }   // JS createBuffer(0) throws -> silence

    int16_t *out = ps_malloc((size_t)out_len * sizeof(int16_t));
    float *phaser_buffer = ps_malloc(1024 * sizeof(float));
    if (!out || !phaser_buffer) { ps_free(out); ps_free(phaser_buffer); *nsamples = 0; return NULL; }

    Biquad bq[3];
    for (int k = 0; k < 3; k++) biquad_lowpass(&bq[k], 1600.0, 1.0, PS_SFX_RATE);
    const float out_scale = (float)(gain / 8.0 / (double)summands);   // /summands, /8, *gain
    int out_i = 0;

    float sample_sum = 0.0f;
    int num_summed = 0;
    const int wave = ps->wave_type;

    for (int t = 0;; ++t) {
        // Repeats
        if (rep_limit != 0 && ++st.rep_time >= rep_limit) sfx_repeat(ps, &st);
        // Arpeggio (single)
        if (st.arp_limit != 0 && t >= st.arp_limit) {
            st.arp_limit = 0;
            st.fperiod *= st.arp_mod;
        }
        // Frequency slide, and frequency slide slide!
        st.fslide += st.fdslide;
        st.fperiod *= st.fslide;
        if (st.fperiod > st.fmaxperiod) st.fperiod = st.fmaxperiod;

        // Vibrato
        float rfperiod = (float)st.fperiod;   // fperiod <= fmaxperiod <= 1e5
        if (vib_amp > 0.0f) {
            vib_phase += vib_speed;
            if (vib_phase >= 6.28318531f) vib_phase -= 6.28318531f;
            rfperiod *= 1.0f + sin_turns(vib_phase * 0.159154943f) * vib_amp;
        }
        int period = (int)rfperiod;
        if (period < 8) period = 8;

        st.square_duty += st.square_slide;
        if (st.square_duty < 0.0f) st.square_duty = 0.0f;
        if (st.square_duty > 0.5f) st.square_duty = 0.5f;

        // Volume envelope
        env_time++;
        if (env_time > env_length[env_stage]) {
            env_time = 1;
            env_stage++;
            while (env_stage < 3 && env_length[env_stage] == 0) env_stage++;
            if (env_stage == 3) break;
        }
        float env_vol;
        if (env_stage == 0) env_vol = (float)env_time * env_inv[0];
        else if (env_stage == 1) env_vol = 1.0f + (1.0f - (float)env_time * env_inv[1]) * punch2;
        else env_vol = 1.0f - (float)env_time * env_inv[2];

        // Phaser step
        float fphase = fphase_f + (float)(t + 1) * fdphase_f;
        float fl = floorf(fphase);
        if (fl < 0) fl = -fl;
        int iphase = fl > 1023.0f ? 1023 : (int)fl;

        if (flthp_d != 0.0) {
            flthp += flthp * flthp_m;
            if (flthp < 0.00001f) flthp = 0.00001f;
            if (flthp > 0.1f) flthp = 0.1f;
        }
        const float flthp_f = flthp;

        // low-pass cutoff for each of the 8 subsamples (fltw *= fltw_d; clamp [0, 0.1])
        float fw[8];
        if (lpf_on) {
            if (fltw >= 0.0f && fltw_d > 0.0 && (fltw_d >= 1.0 || fltw <= 0.1f)) {
                const float base = fltw;
                for (int k = 0; k < 8; k++) {
                    float v = base * fwpow[k + 1];
                    fw[k] = v > 0.1f ? 0.1f : v;
                }
                fltw += fltw * fltw_m8;
                if (fltw > 0.1f) fltw = 0.1f;
            } else {
                for (int k = 0; k < 8; k++) {
                    fltw += fltw * fltw_m;
                    if (fltw < 0.0f) fltw = 0.0f;
                    if (fltw > 0.1f) fltw = 0.1f;
                    fw[k] = fltw;
                }
            }
        }

        // 8x supersampling
        const float fperiod_f = (float)period;
        const float duty = st.square_duty;
        float sample = 0.0f;
        for (int si = 0; si < 8; ++si) {
            float sub_sample;
            phase++;
            if (phase >= period) {
                phase %= period;
                if (wave == NOISE)
                    for (int k = 0; k < 32; ++k) noise_buffer[k] = noise_rand(&nseed);
            }
            // Base waveform
            float fp = (float)phase / fperiod_f;   // true division: keeps fp == duty edges exact
            switch (wave) {
            case SQUARE:   sub_sample = fp < duty ? 0.5f : -0.5f; break;
            case SAWTOOTH: sub_sample = 1.0f - fp * 2; break;
            case SINE:     sub_sample = sin_turns(fp); break;
            case NOISE:    sub_sample = noise_buffer[(phase * 32) / period]; break;
            case TRIANGLE: sub_sample = fabsf(1 - fp * 2) - 1; break;
            default:       sub_sample = fabsf(1 - fp * fp * 2) - 1; break;   // BREAKER
            }
            // Low-pass filter
            float pp = fltp;
            if (lpf_on) {
                fltdp += (sub_sample - fltp) * fw[si];
                fltdp -= fltdp * fltdmp;
            } else {
                fltp = sub_sample;
                fltdp = 0.0f;
            }
            fltp += fltdp;
            // High-pass filter
            fltphp += fltp - pp;
            fltphp -= fltphp * flthp_f;
            sub_sample = fltphp;
            // Phaser
            phaser_buffer[ipp & 1023] = sub_sample;
            sub_sample += phaser_buffer[(ipp - iphase + 1024) & 1023];
            ipp = (ipp + 1) & 1023;
            // final accumulation and envelope application
            sample += sub_sample * env_vol;
        }

        // Accumulate samples appropriately for sample rate
        sample_sum += sample;
        if (++num_summed < summands) continue;
        num_summed = 0;
        out_i = emit(out, out_i, out_len, sample_sum * out_scale, bq);
        sample_sum = 0.0f;
    }
    if (summands > 0) out_i = emit(out, out_i, out_len, sample_sum * out_scale, bq);
    (void)out_i;

    ps_free(phaser_buffer);
    *nsamples = out_len;
    return out;
}

// SoundEffect.generate on cacheSeed's params (sound_vol, sample_rate set as in cacheSeed)
static int16_t *generate_seed(double seed, int *nsamples, int *sampleRate)
{
    Params ps;
    *nsamples = 0;
    if (sampleRate) *sampleRate = PS_SFX_RATE;
    if (!generateFromSeed(seed, &ps)) return NULL;
    ps.sound_vol = SOUND_VOL;
    ps.sample_rate = SAMPLE_RATE;
    uint32_t ns = (uint32_t)fmod(seed, 4294967296.0);
    return sfx_synth(&ps, ns, nsamples);
}

int16_t *ps_sfx_generate(int64_t seed, int *nsamples, int *sampleRate)
{
    return generate_seed((double)seed, nsamples, sampleRate);
}

int16_t *ps_sfx_generate_str(const char *seed, int *nsamples, int *sampleRate)
{
    // Number(seedString) for a run of digits (exact below 2^53, which covers any sane seed)
    double x = 0;
    int nd = 0;
    if (seed)
        for (const char *p = seed; *p >= '0' && *p <= '9'; p++, nd++) x = x * 10 + (*p - '0');
    if (nd == 0) { *nsamples = 0; if (sampleRate) *sampleRate = PS_SFX_RATE; return NULL; }
    return generate_seed(x, nsamples, sampleRate);
}

// Write one 5512.5Hz sample PS_SFX_REPEAT times (as the 22050Hz AudioBuffer does) through the
// play() filter chain, clipping like the WebAudio destination.
static int emit(int16_t *out, int out_i, int out_len, float sample, Biquad *bq)
{
    for (int k = 0; k < PS_SFX_REPEAT && out_i < out_len; k++) {
        float y = biquad_run(&bq[2], biquad_run(&bq[1], biquad_run(&bq[0], sample)));
        if (y > 1.0f) y = 1.0f;
        if (y < -1.0f) y = -1.0f;
        out[out_i++] = (int16_t)lrintf(y * 32767.0f);
    }
    return out_i;
}
