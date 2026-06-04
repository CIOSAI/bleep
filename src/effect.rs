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
        f32,
    ) -> f32,
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
        input: f32
    | {
        let mut accum = input;

        // is sample_rate ever large enough to have issues with floating point precision?
        let samples_of_time = 2.max(BUFFER_SIZE.min(((params[1]*(*sample_rate as f32)) as usize)*2));
        let delay_sound = buffer[(*pointer+1)%samples_of_time];

        accum += delay_sound*params[0];

        buffer[*pointer] = accum;
        *pointer = (*pointer + 1)%samples_of_time;

        accum 
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
        input: f32
    | {
        (input*params[0]*64.0).clamp(-1.0, 1.0)*params[1]
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
