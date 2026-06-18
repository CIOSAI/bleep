use std::f32::consts::{TAU};

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
    ) -> Vec<f32>,
    pub init: fn() -> GeneratorData,
}
pub struct GeneratorData {
    pub params: [f32; MAXIMUM_PARAM_INDEX],
}
pub struct Generator {
    pub definition: &'static GeneratorDefinition,
    pub data: GeneratorData,
}

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
    | {
        let size = *sample_rate;
        let mut result:Vec<f32> = Vec::new();

        for i in 0..size {
            let t = ((i/2) as f32)/(*sample_rate as f32);

            let pitch = params[0];
            let volume = params[1];
            let decay = 64.0-params[2]*63.0;

            let mut val;
            {
                val = (t*pitch*TAU).sin();
                val *= f32::exp(-t.fract() * decay) * volume;
            }

            result.push(val);
        }

        result
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
    | {
        let size = *sample_rate;
        let mut result:Vec<f32> = Vec::new();
        for i in 0..size {
            let is_left = i%2==0;
            let t = ((i/2) as f32)/(*sample_rate as f32);

            let pitch = params[0];
            let volume = params[1];
            let decay = 64.0-params[2]*63.0;

            let mut val;
            {
                let fm = (t * (if is_left {30.0} else {40.0})).sin();
                val = (t*pitch+fm*0.3).fract()*2.0-1.0;
                val *= f32::exp(-t.fract() * decay) * volume;
            }

            result.push(val);
        }

        result
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

