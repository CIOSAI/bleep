use crate::util;
use std::f32::consts::{TAU};
use std::hash::{Hash, Hasher, DefaultHasher};

pub const MAXIMUM_PARAM_INDEX:usize = 64;

// let's keep param[0] as pitch until i find something better TODO
// controls for this??

pub struct GeneratorDefinition {
    pub title: &'static str,
    pub param_names: [&'static str; MAXIMUM_PARAM_INDEX],
    pub param_count: usize,
    pub apply: fn(
        &u32,
        [f32; MAXIMUM_PARAM_INDEX], 
        &u128,
        &bool,
    ) -> ([f32;util::CHUNK_SIZE], bool),
    pub init: fn() -> GeneratorData,
}
pub struct GeneratorData {
    pub params: [f32; MAXIMUM_PARAM_INDEX],
}
pub struct Generator {
    pub definition: &'static GeneratorDefinition,
    pub data: GeneratorData,
}

// TODO: by not distinguishing attack moment and release moment, we also can't have proper sustain
pub const SINE_OSC:GeneratorDefinition = GeneratorDefinition {
    title: "sine osc",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "pitch";
        a[1] = "volume";
        a[2] = "decay";
        a
    },
    param_count: 3,
    apply: |
        sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        chunk_index: &u128,
        is_pressed: &bool,
    | {
        let mut result:[f32;util::CHUNK_SIZE] = [0.0; util::CHUNK_SIZE];

        if *is_pressed {
            for i in 0..util::CHUNK_SIZE {
                let ti = (*chunk_index as usize)*util::CHUNK_SIZE + i;
                let t = ((ti/2) as f32)/(*sample_rate as f32);

                let pitch = params[0];
                let volume = params[1];
                let decay = 64.0-params[2]*63.0;

                let mut val;
                {
                    val = (t*pitch*TAU).sin();
                    val *= f32::exp(-t.fract() * decay) * volume;
                }

                result[i] = val;
            }
        }

        (result, chunk_index*(util::CHUNK_SIZE as u128) > (*sample_rate as u128))
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 440.0;
        a[1] = 0.5;
        a[2] = 0.6;
        GeneratorData {
            params: a,
        }
    }
};

// TODO: phase issues (again due to not distinguishing attack moment and release moment)
pub const DETUNED_SAW:GeneratorDefinition = GeneratorDefinition {
    title: "detuned saw",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "pitch";
        a[1] = "volume";
        a[2] = "decay";
        a
    },
    param_count: 3,
    apply: |
        sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        chunk_index: &u128,
        is_pressed: &bool,
    | {
        let mut result:[f32;util::CHUNK_SIZE] = [0.0; util::CHUNK_SIZE];
        let mut finished = !is_pressed;
        finished = finished && chunk_index*(util::CHUNK_SIZE as u128) > (*sample_rate as u128);

        for i in 0..util::CHUNK_SIZE {
            let is_left = i%2==0;
            let ti = (*chunk_index as usize)*util::CHUNK_SIZE + i;
            let t = ((ti/2) as f32)/(*sample_rate as f32);

            let pitch = params[0];
            let volume = params[1];
            let decay = 64.0-params[2]*63.0;

            let mut val;
            {
                let fm = (t * (if is_left {30.0} else {40.0})).sin();
                val = (t*pitch+fm*0.3).fract()*2.0-1.0;
                val *= volume;
                val *= if *is_pressed { 1.0 } else { f32::exp(-t.fract() * decay) };
            }

            result[i] = val;
        }

        (result, finished)
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 440.0;
        a[1] = 0.25;
        a[2] = 0.6;
        GeneratorData {
            params: a,
        }
    }
};

pub const WHITE_NOISE:GeneratorDefinition = GeneratorDefinition {
    title: "white noise",
    param_names: {
        let mut a = ["";MAXIMUM_PARAM_INDEX];
        a[0] = "N/A";
        a[1] = "volume";
        a
    },
    param_count: 3,
    apply: |
        _sample_rate: &u32,
        params:[f32; MAXIMUM_PARAM_INDEX], 
        chunk_index: &u128,
        is_pressed: &bool,
    | {
        let mut result:[f32;util::CHUNK_SIZE] = [0.0; util::CHUNK_SIZE];

        if *is_pressed {
            let mut hasher = DefaultHasher::new();
            (*chunk_index).hash(&mut hasher);
            for i in 0..util::CHUNK_SIZE {
                let is_left = i%2==0;

                let volume = params[1];

                let mut val;
                {
                    i.hash(&mut hasher);
                    is_left.hash(&mut hasher);
                    val = (hasher.finish() as f32)/(u64::MAX as f32) * 2.0 - 1.0;
                    val *= volume;
                }

                result[i] = val;
            }

            return (result, false);
        }
        else {
            return (result, true);
        }
    },
    init: || {
        let mut a = [0.0;MAXIMUM_PARAM_INDEX];
        a[0] = 440.0;
        a[1] = 0.25;
        a[2] = 0.6;
        GeneratorData {
            params: a,
        }
    }
};
