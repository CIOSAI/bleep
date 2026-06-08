use crate::util;

pub const BUFFER_SIZE:usize = 1<<16;
pub const MAXIMUM_PARAM_INDEX:usize = 64;
pub struct EffectDefinition {
    pub title: &'static str,
    pub param_names: [&'static str; MAXIMUM_PARAM_INDEX],
    pub param_count: usize,
    pub apply: fn(
        &u32,
        [f32; MAXIMUM_PARAM_INDEX], 
        &mut [f32; BUFFER_SIZE], 
        &mut usize, 
        &[f32;util::CHUNK_SIZE],
    ) -> [f32;util::CHUNK_SIZE],
    pub init: fn() -> EffectData,
}
pub struct EffectData {
    pub params: [f32; MAXIMUM_PARAM_INDEX],
    pub buffer: [f32; BUFFER_SIZE],
    pub buffer_pointer: usize,
}
pub struct Effect {
    pub definition: &'static EffectDefinition,
    pub data: EffectData,
}

// TODO: residual signal from longer delay time
// delay time used to be 0.8, changes to 0.4
// in this case the data from 0.4-0.8 are not overwritten
// later when the delay time moves back to 0.8, it plays
// sounds from waaay before
pub const DELAY:EffectDefinition = EffectDefinition {
    title: "delay",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "volume";
        a[1] = "time";
        a
    },
    param_count: 2,
    apply: |
        sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        buffer: &mut [f32; BUFFER_SIZE], 
        pointer: &mut usize, 
        input: &[f32;util::CHUNK_SIZE]
    | {
        let mut result = [0.0;util::CHUNK_SIZE];
        for i in 0..util::CHUNK_SIZE {
            let mut accum = input[i];

            // is sample_rate ever large enough to have issues with floating point precision?
            let samples_of_time = 2.max(BUFFER_SIZE.min(((params[1]*(*sample_rate as f32)) as usize)*2));
            let delay_sound = buffer[(*pointer+1)%samples_of_time];

            accum += delay_sound*params[0];

            buffer[*pointer] = accum;
            *pointer = (*pointer + 1)%samples_of_time;

            result[i] = accum;
        }

        result
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 0.0;
        a[1] = 0.25;
        EffectData {
            params: a,
            buffer: [0.0; BUFFER_SIZE],
            buffer_pointer: 0,
        }
    }
};

pub const HARDCLIP:EffectDefinition = EffectDefinition {
    title: "hard clip",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "pregain";
        a[1] = "postgain";
        a
    },
    param_count: 2,
    apply: |
        _sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        _buffer: &mut [f32; BUFFER_SIZE], 
        _pointer: &mut usize, 
        input: &[f32;util::CHUNK_SIZE]
    | {
        let mut result = [0.0;util::CHUNK_SIZE];
        for i in 0..util::CHUNK_SIZE {
            result[i] = (input[i]*params[0]*64.0).clamp(-1.0, 1.0)*params[1]
        }
        result
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 1.0/64.0;
        a[1] = 1.0;
        EffectData {
            params: a,
            buffer: [0.0; BUFFER_SIZE],
            buffer_pointer: 0,
        }
    }
};

pub const PAN:EffectDefinition = EffectDefinition {
    title: "panning",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "pan";
        a
    },
    param_count: 1,
    apply: |
        _sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        _buffer: &mut [f32; BUFFER_SIZE], 
        _pointer: &mut usize, 
        input: &[f32;util::CHUNK_SIZE]
    | {
        let mut result = [0.0;util::CHUNK_SIZE];
        // TODO: there may be a better way to write this math
        let direct = [if params[0]<0.5 {params[0]+0.5} else {2.0-params[0]*2.0}, 
                      if params[0]<0.5 {params[0]*2.0} else {1.5-params[0]}];
        let opposite = [(0.5-params[0]).max(0.0), (params[0]-0.5).max(0.0)];
        for i in 0..(util::CHUNK_SIZE/2) {
            result[i*2+0] = input[i*2+0]*direct[0] + input[i*2+1]*opposite[0];
            result[i*2+1] = input[i*2+0]*opposite[1] + input[i*2+1]*direct[1];
        }
        result
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 0.5;
        EffectData {
            params: a,
            buffer: [0.0; BUFFER_SIZE],
            buffer_pointer: 0,
        }
    }
};
