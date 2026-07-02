// with much help from https://whoisryosuke.com/blog/2026/creating-a-daw-in-rust/
// rust audio discord

use core::f32;
use std::collections::HashMap;
use std::sync::Arc;
use std::time;

use clap::Parser;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SizedSample, Sample, I24
};
use ringbuf::{
    HeapRb, SharedRb, traits::{Producer, Consumer, Split}, wrap::caching::{Caching}, storage::{Heap}, 
};
use device_query::{DeviceQuery, DeviceState, Keycode};

mod util;
mod effect;
mod generator;

const GENERATOR_BANK:[&generator::GeneratorDefinition;3] = [&generator::SINE_OSC, &generator::DETUNED_SAW, &generator::WHITE_NOISE];
const EFFECT_BANK:[&effect::EffectDefinition;6] = [&effect::DELAY, &effect::HARDCLIP, &effect::PAN, &effect::LOWPASS, &effect::HIGHPASS, &effect::EQ];
const SMOOTHING_SECONDS:f32 = 0.023;
const DECLICK_SIZE: usize = 8;
const BUFFER_REGION_SIZE: usize = 2048;

#[derive(Parser, Debug)]
#[command(version, about = "CPAL beep example", long_about = None)]
struct Opt {
    /// The audio device to use
    #[arg(short, long)]
    device: Option<String>,

    #[arg(short, long)]
    #[allow(dead_code)]
    jack: bool,
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::parse();

    let host = cpal::default_host();

    let device = if let Some(device) = opt.device {
        let id = &device.parse().expect("failed to parse device id");
        host.device_by_id(id)
    } else {
        host.default_output_device()
    }
    .expect("failed to find output device");
    println!("Output device: {}", device.id()?);

    let config = device.default_output_config().unwrap();
    println!("Default output config: {config:?}");

    match config.sample_format() {
        cpal::SampleFormat::I8 => run::<i8>(&device, &config.into()),
        cpal::SampleFormat::I16 => run::<i16>(&device, &config.into()),
        cpal::SampleFormat::I24 => run::<I24>(&device, &config.into()),
        cpal::SampleFormat::I32 => run::<i32>(&device, &config.into()),
        // cpal::SampleFormat::I48 => run::<I48>(&device, &config.into()),
        cpal::SampleFormat::I64 => run::<i64>(&device, &config.into()),
        cpal::SampleFormat::U8 => run::<u8>(&device, &config.into()),
        cpal::SampleFormat::U16 => run::<u16>(&device, &config.into()),
        // cpal::SampleFormat::U24 => run::<U24>(&device, &config.into()),
        cpal::SampleFormat::U32 => run::<u32>(&device, &config.into()),
        // cpal::SampleFormat::U48 => run::<U48>(&device, &config.into()),
        cpal::SampleFormat::U64 => run::<u64>(&device, &config.into()),
        cpal::SampleFormat::F32 => run::<f32>(&device, &config.into()),
        cpal::SampleFormat::F64 => run::<f64>(&device, &config.into()),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    }
}

pub struct NoteData {
    instrument: usize,
    is_pressed: bool,
    pitch: f32,
    current_chunk: u128,
}

pub struct AudioSample {
    sample: [f32;util::CHUNK_SIZE],
    position: usize,
}

#[derive(Clone)]
enum AudioCommand {
    NewGenerator(&'static generator::GeneratorDefinition),
    Press(usize, Keycode, f32),
    Release(Keycode),
    NewEffect(&'static effect::EffectDefinition),
    DelEffect(usize),
    SetParam(usize,usize,f32),
}

pub struct Player {
    playing_keys: HashMap<Keycode, NoteData>,
    playing_samples: Vec<AudioSample>,
    consumer: Caching<Arc<SharedRb<Heap<AudioCommand>>>, false, true>,
    // put sounds in here first, for doing whatever final adjustment before streaming
    buffer_region: [f32;BUFFER_REGION_SIZE],
    buffer_region_pointer: usize,
    generators: Vec<generator::Generator>,
    effect_stack: Vec<effect::Effect>,
    effect_wetness: Vec<f32>,
}
impl Player {
    pub fn process<T:Sample>(&mut self, data: &mut [T], _channels: u32, sample_rate: u32)
        where T: FromSample<f32>
    {
        while let Some(command) = self.consumer.try_pop() {
            match command {
                AudioCommand::NewGenerator(def) => {
                    self.generators.push(generator::Generator {
                        definition: def,
                        data: (def.init)(),
                    });
                },
                AudioCommand::Press(generator, key, pitch) => {
                    if generator>=self.generators.len() { return };
                    match self.playing_keys.get_mut(&key) {
                        Some(note_data) => {
                            note_data.instrument = generator;
                            note_data.is_pressed = true;
                            note_data.current_chunk = 0;
                        },
                        None => {
                            self.playing_keys.insert(key.clone(), NoteData {
                                instrument: generator,
                                is_pressed: true,
                                pitch: pitch,
                                current_chunk: 0,
                            });
                        },
                    }
                },
                AudioCommand::Release(key) => {
                    match self.playing_keys.get_mut(&key) {
                        Some(note_data) => {
                            note_data.is_pressed = false;
                            note_data.current_chunk = 0;
                        },
                        None => {}
                    }
                },
                AudioCommand::NewEffect(def) => {
                    self.effect_stack.push(effect::Effect {
                        definition: def,
                        data: (def.init)(),
                    });
                    self.effect_wetness.push(1.0);
                },
                AudioCommand::DelEffect(index) => {
                    self.effect_stack.remove(index);
                    self.effect_wetness.remove(index);
                },
                AudioCommand::SetParam(index, key, value) => {
                    if key>=effect::MAXIMUM_PARAM_INDEX+1 { return };
                    if index>=self.effect_stack.len() { return };
                    if key==0 {
                        self.effect_wetness[index] = value;
                    }
                    else {
                        self.effect_stack[index].data.params[key-1] = value;
                    }
                },
            }
        }

        for chunk in data.chunks(util::CHUNK_SIZE) {
            // create samples
            let mut kill_list:Vec<Keycode> = Vec::new();
            for (key, note_data) in self.playing_keys.iter_mut() {
                let generator = &mut self.generators[note_data.instrument];
                (*generator).data.params[0] = note_data.pitch;
                let (sample_chunk, finished) = (generator.definition.apply)(
                    &sample_rate,
                    generator.data.params,
                    &note_data.current_chunk,
                    &note_data.is_pressed, // couldve been a param i guess?
                );
                note_data.current_chunk += 1;
                self.playing_samples.push(AudioSample {
                    sample: sample_chunk,
                    position: 0
                });
                if (!note_data.is_pressed) && finished {
                    kill_list.push(key.clone());
                }
            }
            for victim in kill_list {
                self.playing_keys.remove(&victim);
            }
            // create sound
            let mut signal = [0.0;util::CHUNK_SIZE];
            for i in 0..chunk.len() {
                let mut accum:f32 = 0.0;
                self.playing_samples.retain_mut(|sound| {
                    if sound.position>=sound.sample.len() {
                        false
                    }
                    else {
                        let num = sound.sample.get(sound.position).unwrap_or(&0.0);
                        sound.position += 1;
                        accum += num;
                        true
                    }
                });
                signal[i] = accum;
            }
            // effects
            for (i, effect) in self.effect_stack.iter_mut().enumerate() {
                let wetness = self.effect_wetness[i];
                let wet_signal:[f32;util::CHUNK_SIZE] = (effect.definition.apply)(
                    &sample_rate,
                    effect.data.params,
                    &mut effect.data.buffer,
                    &mut effect.data.buffer_pointer,
                    &signal,
                );
                for i in 0..util::CHUNK_SIZE {
                    signal[i] = signal[i]*(1.0-wetness)+wet_signal[i]*wetness;
                }
            }
            let temp = self.buffer_region_pointer;
            // write
            for i in 0..chunk.len() {
                self.buffer_region[self.buffer_region_pointer] = signal[i].clamp(-1.0, 1.0);
                self.buffer_region_pointer = (self.buffer_region_pointer+1) % BUFFER_REGION_SIZE;
            }

            // declick
            let coeff_b = (-1.0 / (SMOOTHING_SECONDS * (sample_rate as f32)).max(f32::MIN)).exp();

            let declick_start = if temp>=DECLICK_SIZE/2 {
                (temp - DECLICK_SIZE/2) % BUFFER_REGION_SIZE
            } else { self.buffer_region.len().strict_add_signed((temp as isize)-((DECLICK_SIZE/2) as isize)) };

            let mut declick_samples = [self.buffer_region[declick_start],
                                       self.buffer_region[declick_start+1]
            ];
            for i in (1..(DECLICK_SIZE/2)).map(|i| declick_start+i*2) {
                let smooth = |a,b| a*(1.0-coeff_b)+b*coeff_b;
                declick_samples[0] = smooth(declick_samples[0], self.buffer_region[i % BUFFER_REGION_SIZE]);
                declick_samples[0] = smooth(declick_samples[1], self.buffer_region[(i+1) % BUFFER_REGION_SIZE]);
                self.buffer_region[i % BUFFER_REGION_SIZE] = declick_samples[0];
                self.buffer_region[(i+1) % BUFFER_REGION_SIZE] = declick_samples[1];
            }
        }

        // stream out
        for i in 0..data.len() {
            data[i] = Sample::from_sample(self.buffer_region[self.buffer_region_pointer+i]);
        }
    }
}
pub struct Instrument {
    producer: Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
    player: Player,
}
impl Instrument {
    pub fn new() -> Self {
        let ring = HeapRb::<AudioCommand>::new(64);
        let (producer, consumer) = ring.split();

        Self {
            producer: producer,
            player: Player {
                playing_keys: HashMap::new(),
                playing_samples: Vec::new(),
                consumer: consumer,
                buffer_region: [0.0;BUFFER_REGION_SIZE],
                buffer_region_pointer: 0,
                generators: Vec::new(),
                effect_stack: Vec::new(),
                effect_wetness: Vec::new(),
            }
        }
    }
}

pub enum Modes {
    PERFORM,
    EDIT,
}

struct GUI {
    throttle_ms: u128,
    last_sent_slider: time::SystemTime,
    edit_column: usize,
    current_instrument: usize,
    current_effect: usize,
    current_param: usize,
    current_to_add: usize,
    current_octave: i32,
    mode: Modes,
    effect_stack: Vec<&'static effect::EffectDefinition>,
    effect_params: Vec<Vec<f32>>,
    effect_wetness: Vec<f32>,
}

pub fn run<T>(device: &cpal::Device, config: &cpal::StreamConfig) -> Result<(), anyhow::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let err_fn = |err| eprintln!("an error occurred on stream: {err}");

    let instrument:Instrument = Instrument::new();
    let mut prod = instrument.producer;
    let mut player = instrument.player;

    _ = prod.try_push(AudioCommand::NewGenerator(GENERATOR_BANK[0]));
    _ = prod.try_push(AudioCommand::NewGenerator(GENERATOR_BANK[1]));
    _ = prod.try_push(AudioCommand::NewGenerator(GENERATOR_BANK[2]));

    let channels = config.channels.clone() as u32; 
    let sample_rate = config.sample_rate.clone();

    let listen = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            player.process(data, channels, sample_rate);
        },
        err_fn,
        None,
    )?;
    listen.play()?;

    println!("press ESC key to stop program");

    let device_reader = DeviceState::new();
    let mut pressed:Vec<Keycode> = Vec::new();

    let edo12fromC = |f:f32| 440.0 * f32::powf(2.0, (3.0+f)/12.0);
    let key_matrix = [vec![
        Keycode::Key1,Keycode::Key2,Keycode::Key3,Keycode::Key4,Keycode::Key5,Keycode::Key6,Keycode::Key7,Keycode::Key8,Keycode::Key9,Keycode::Key0,Keycode::Minus,Keycode::Equal,
    ],vec![
        Keycode::Q,Keycode::W,Keycode::E,Keycode::R,Keycode::T,Keycode::Y,Keycode::U,Keycode::I,Keycode::O,Keycode::P,Keycode::LeftBracket,Keycode::RightBracket,
    ],vec![
        Keycode::A,Keycode::S,Keycode::D,Keycode::F,Keycode::G,Keycode::H,Keycode::J,Keycode::K,Keycode::L,Keycode::Semicolon,Keycode::Apostrophe,
    ],vec![
        Keycode::Z,Keycode::X,Keycode::C,Keycode::V,Keycode::B,Keycode::N,Keycode::M,Keycode::Comma,Keycode::Dot,Keycode::Slash,
    ],];
    const NOTE_OF_TOPLEFT:f32 = -7.0;
    let mut piano:HashMap<Keycode, f32> = HashMap::new();
    // wicki-hayden for qwertyUS
    for (i,row) in key_matrix.iter().enumerate() {
        for (j,key) in row.iter().enumerate() {
            piano.insert(key.clone(), edo12fromC(NOTE_OF_TOPLEFT-((i*5) as f32)+((j*2) as f32)));
        }
    }

    let mut gui = GUI {
        throttle_ms: 50,
        last_sent_slider: time::SystemTime::now(),
        edit_column: 1,
        current_instrument: 0,
        current_effect: 0,
        current_param: 0,
        current_to_add: 0,
        current_octave: 0,
        mode: Modes::PERFORM,
        effect_stack: Vec::new(),
        effect_params: Vec::new(),
        effect_wetness: Vec::new(),
    };

    let redraw = |gui: &GUI| {
        let GUI {
            throttle_ms: _,
            last_sent_slider: _,
            edit_column,
            current_instrument,
            current_effect,
            current_param,
            current_to_add,
            current_octave,
            mode,
            effect_stack,
            effect_params,
            effect_wetness,
        } = gui;

        print!("\n\n\n");
        println!("TAB switch mode | h ← | k ↑ | j | ↓ l → | mouseX effect slider(hug the top side)");
        match mode {
            Modes::PERFORM => {
                println!("PERFORM MODE");
                println!("octave {}, arrow ← → to change", current_octave);
                println!("{}", GENERATOR_BANK[*current_instrument].title);
                for i in 0..effect_stack.len() {
                    println!("  {}", effect_stack[i].title);
                    print!("  ");
                    for j in 0..effect_stack[i].param_count+1 {
                        let editing = i==*current_effect && j==*current_param;
                        print!("{}\t{}\t{}",
                               if editing {">"} else {" "},
                               if j==0 {"wet"} else {effect_stack[i].param_names[j-1]},
                               if j==0 {effect_wetness[i]} else {effect_params[i][j-1]}
                        );
                    }
                    println!("");
                }
            },
            Modes::EDIT => {
                println!("EDIT MODE");
                println!("current effect stack:\tavailable effects:\tinstruments:");
                println!("press delete to remove:\tpress enter to add:\tJ and K to choose:\n");
                for i in 0..usize::max(effect_stack.len(),EFFECT_BANK.len()) {
                    let stack_hovering = *edit_column==0 && i==*current_effect;
                    print!("{}\t{}", 
                             if stack_hovering {">"} else {" "},
                             if i<effect_stack.len() {effect_stack[i].title} else {""}
                    );
                    print!("\t");
                    let bank_hovering = *edit_column==1 && i==*current_to_add;
                    print!("{}\t{}",
                             if bank_hovering {">"} else {" "},
                             if i<EFFECT_BANK.len() {EFFECT_BANK[i].title} else {""}
                    );
                    print!("\t");
                    let instrument_hovering = *edit_column==2 && i==*current_instrument;
                    print!("{}\t{}",
                             if instrument_hovering {">"} else {" "},
                             if i<GENERATOR_BANK.len() {GENERATOR_BANK[i].title} else {""}
                    );
                    println!("");
                }
            },
        };
    };

    redraw(&gui);

    let add_effect = |
        prod: &mut Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
        gui: &mut GUI,
        to_add: &'static effect::EffectDefinition,
    | {
        let effects = &mut gui.effect_stack;
        let wetness = &mut gui.effect_wetness;
        let params = &mut gui.effect_params;
        effects.push(to_add);
        wetness.push(1.0);
        let default_values = (to_add.init)().params;
        let mut vec = Vec::new();
        for i in 0..to_add.param_count { vec.push(default_values[i]); }
        params.push(vec);
        let _ = prod.try_push(AudioCommand::NewEffect(to_add));
    };

    let del_effect = |
        prod: &mut Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
        gui: &mut GUI,
        index: usize,
    | {
        let effects = &mut gui.effect_stack;
        let wetness = &mut gui.effect_wetness;
        let params = &mut gui.effect_params;
        if index<=effects.len() {
            effects.remove(index);
            wetness.remove(index);
            params.remove(index);
            let _ = prod.try_push(AudioCommand::DelEffect(index));
        }
        if gui.current_effect>=effects.len() {
            gui.current_effect = if effects.len()==0 { 0 } else { effects.len()-1 };
        }
    };

    let set_param = |
        prod: &mut Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
        gui: &mut GUI,
        to_value: f32,
    | {
        let params = &mut gui.effect_params;
        let wetness = &mut gui.effect_wetness;
        let which_effect = gui.current_effect;
        let which_param = gui.current_param;

        let _ = prod.try_push(
            AudioCommand::SetParam(which_effect, which_param, to_value)
        );
        if which_param==0 {
            wetness[which_effect] = to_value;
        }
        else if which_effect<params.len() && which_param-1<params[which_effect].len() {
            params[which_effect][which_param-1] = to_value;
        }
    };


    loop {
        let keys = device_reader.get_keys();

        let just_pressed: Vec<Keycode> = keys
            .iter()
            .filter(|k| !pressed.contains(k))
            .cloned()
            .collect();
        let just_released: Vec<Keycode> = pressed
            .iter()
            .filter(|k| !keys.contains(k))
            .cloned()
            .collect();

        for (k,v) in &piano {
            if just_pressed.contains(&k) {
                let _ = prod.try_push(AudioCommand::Press(gui.current_instrument, k.clone(), *v * 2.0_f32.powi(gui.current_octave)));
            }
            if just_released.contains(&k) {
                let _ = prod.try_push(AudioCommand::Release(k.clone()));
            }
        }

        match gui.mode {
            Modes::PERFORM => {
                if just_pressed.contains(&Keycode::Left) {
                    gui.current_octave -= 1;
                }
                if just_pressed.contains(&Keycode::Right) {
                    gui.current_octave += 1;
                }

                if !gui.effect_params.is_empty() {
                    if just_pressed.contains(&Keycode::K) {
                        gui.current_effect = if gui.current_effect>0 { gui.current_effect-1 } else { gui.effect_params.len()-1 };
                    }
                    if just_pressed.contains(&Keycode::J) {
                        gui.current_effect = if gui.current_effect<gui.effect_params.len()-1 { gui.current_effect+1 } else { 0 };
                    }

                    let param_count = gui.effect_params[gui.current_effect].len()+1;
                    if gui.current_param>=param_count { gui.current_param = param_count-1; }
                    if just_pressed.contains(&Keycode::H) {
                        gui.current_param = if gui.current_param>0 { gui.current_param-1 } else { param_count-1 };
                    }
                    if just_pressed.contains(&Keycode::L) {
                        gui.current_param = if gui.current_param<param_count-1 { gui.current_param+1 } else { 0 };
                    }
                }
                
                if !gui.effect_stack.is_empty() && gui.last_sent_slider.elapsed()
                    .is_ok_and(|dur| dur.as_millis()>gui.throttle_ms) {
                    let mouse = device_reader.get_mouse().coords;
                    // only change values when it's on the top side
                    if mouse.1<120 {
                        let slider = (mouse.0 as f32) / 1920.0;
                        set_param(
                            &mut prod,
                            &mut gui,
                            slider,
                        );
                        redraw(&gui);
                        gui.last_sent_slider = time::SystemTime::now();
                    }
                }

                if just_pressed.contains(&Keycode::Tab) {
                    gui.mode = Modes::EDIT;
                }
            },
            Modes::EDIT => {
                if just_pressed.contains(&Keycode::H) {
                    gui.edit_column = if gui.edit_column>0 { gui.edit_column-1 } else { 2 };
                }
                if just_pressed.contains(&Keycode::L) {
                    gui.edit_column = if gui.edit_column<2 { gui.edit_column+1 } else { 0 };
                }
                match gui.edit_column {
                    0 => {
                        if !gui.effect_stack.is_empty() {
                            if just_pressed.contains(&Keycode::K) {
                                gui.current_effect = if gui.current_effect>0 { gui.current_effect-1 } else { gui.effect_stack.len()-1 };
                            }
                            if just_pressed.contains(&Keycode::J) {
                                gui.current_effect = if gui.current_effect<gui.effect_stack.len()-1 { gui.current_effect+1 } else { 0 };
                            }
                            if just_pressed.contains(&Keycode::Delete) {
                                let current_effect = gui.current_effect;
                                del_effect(
                                    &mut prod,
                                    &mut gui,
                                    current_effect
                                );
                            }
                        }
                    },
                    1 => {
                        if just_pressed.contains(&Keycode::K) {
                            gui.current_to_add = if gui.current_to_add>0 { gui.current_to_add-1 } else { EFFECT_BANK.len()-1 };
                        }
                        if just_pressed.contains(&Keycode::J) {
                            gui.current_to_add = if gui.current_to_add<EFFECT_BANK.len()-1 { gui.current_to_add+1 } else { 0 };
                        }
                        if just_pressed.contains(&Keycode::Enter) {
                            let current_to_add = gui.current_to_add;
                            add_effect(
                                &mut prod,
                                &mut gui,
                                EFFECT_BANK[current_to_add]
                            );
                        }
                    },
                    2 => {
                        if just_pressed.contains(&Keycode::K) {
                            gui.current_instrument = if gui.current_instrument>0 { gui.current_instrument-1 } else { GENERATOR_BANK.len()-1 };
                        }
                        if just_pressed.contains(&Keycode::J) {
                            gui.current_instrument = if gui.current_instrument<GENERATOR_BANK.len()-1 { gui.current_instrument+1 } else { 0 };
                        }
                    },
                    _ => { },
                }

                if just_pressed.contains(&Keycode::Tab) {
                    gui.mode = Modes::PERFORM;
                }
            },
        };

        // stuff was entered to the terminal, redraw
        if !just_pressed.is_empty() {
            redraw(&gui);
        }

        if keys.contains(&Keycode::Escape) {
            break;
        }

        pressed = keys;
    }

    Ok(())
}
