// with much help from https://whoisryosuke.com/blog/2026/creating-a-daw-in-rust/

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

pub struct OneSound {
    note_pitch: i8,
    position: usize,
}
// TODO: allow for removing effects
#[derive(Clone)]
enum AudioCommand {
    Bell(i8),
    NewEffect(&'static effect::EffectDefinition),
    SetParam(usize,usize,f32),
}

pub struct Player {
    cached_sounds: HashMap<i8, Arc<Vec<f32>>>,
    playing: Vec<OneSound>,
    consumer: Caching<Arc<SharedRb<Heap<AudioCommand>>>, false, true>,
    effect_stack: Vec<effect::Effect>,
    effect_wetness: Vec<f32>,
}
impl Player {
    pub fn process<T:Sample>(&mut self, data: &mut [T], channels: u32, sample_rate: u32) 
        where T: FromSample<f32>
    {
        while let Some(command) = self.consumer.try_pop() {
            match command {
                AudioCommand::Bell(note_name) => {
                    self.playing.push(OneSound { note_pitch: note_name.clone(), position: 0 });

                    if !self.cached_sounds.contains_key(&note_name) {
                        let size = (sample_rate/2) * (channels);
                        let mut buffer:Vec<f32> = Vec::new();
                        for i in 0..size {
                            let is_left = i%channels==0;
                            let t = ((i/channels) as f32)/(sample_rate as f32);

                            let pitch = Instrument::name_to_pitch(&note_name);
                            let decay = 40.0;
                            let volume = 0.2;

                            let mut val;
                            {
                                val = (t*pitch*f32::consts::TAU).sin();
                                val *= f32::exp(-t.fract() * decay) * volume;
                            }

                            buffer.push(val);
                        }

                        self.cached_sounds.insert(note_name.clone(), Arc::new(buffer));
                    }
                },
                AudioCommand::NewEffect(def) => {
                    self.effect_stack.push(effect::Effect {
                        definition: def,
                        data: (def.init)(),
                    });
                    self.effect_wetness.push(1.0);
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

        for chunk in data.chunks_mut(util::CHUNK_SIZE) {
            let mut signal = [0.0;util::CHUNK_SIZE];
            for i in 0..util::CHUNK_SIZE {
                let mut accum:f32 = 0.0;
                self.playing.retain_mut(|sound| {
                    match self.cached_sounds.get(&sound.note_pitch) {
                        Some(buffer) => {
                            if sound.position>=buffer.len() {
                                false
                            }
                            else {
                                let num = buffer.get(sound.position).unwrap_or(&0.0);
                                sound.position += 1;
                                accum += num;
                                true
                            }
                        },
                        None => { false }
                    }
                });
                signal[i] = accum;
            }
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
            for i in 0..util::CHUNK_SIZE {
                chunk[i] = Sample::from_sample(signal[i].clamp(-1.0, 1.0));
            }
        }
    }
}
pub struct Instrument {
    producer: Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
    player: Player,
}
impl Instrument {
    pub fn name_to_pitch(name: &i8) -> f32 {
        match name {
            0 => 440.0 * f32::powf(2.0, 3.0/12.0),
            1 => 440.0 * f32::powf(2.0, 4.0/12.0),
            2 => 440.0 * f32::powf(2.0, 5.0/12.0),
            3 => 440.0 * f32::powf(2.0, 6.0/12.0),
            4 => 440.0 * f32::powf(2.0, 7.0/12.0),
            5 => 440.0 * f32::powf(2.0, 8.0/12.0),
            6 => 440.0 * f32::powf(2.0, 9.0/12.0),
            7 => 440.0 * f32::powf(2.0, 10.0/12.0),
            8 => 440.0 * f32::powf(2.0, 11.0/12.0),
            9 => 440.0 * f32::powf(2.0, 12.0/12.0),
            10 => 440.0 * f32::powf(2.0, 13.0/12.0),
            11 => 440.0 * f32::powf(2.0, 14.0/12.0),
            12 => 440.0 * f32::powf(2.0, 15.0/12.0),
            _ => 440.0
        }
    }

    pub fn new() -> Self {
        let ring = HeapRb::<AudioCommand>::new(64);
        let (producer, consumer) = ring.split();

        Self {
            producer: producer,
            player: Player {
                cached_sounds: HashMap::new(),
                playing: Vec::new(),
                consumer: consumer,
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

pub fn run<T>(device: &cpal::Device, config: &cpal::StreamConfig) -> Result<(), anyhow::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let err_fn = |err| eprintln!("an error occurred on stream: {err}");

    let instrument:Instrument = Instrument::new();
    let mut prod = instrument.producer;
    let mut player = instrument.player;

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

    let mut piano:HashMap<Keycode, AudioCommand> = HashMap::new();
    piano.insert(Keycode::Q, AudioCommand::Bell(0));
    piano.insert(Keycode::Key2, AudioCommand::Bell(1));
    piano.insert(Keycode::W, AudioCommand::Bell(2));
    piano.insert(Keycode::Key3, AudioCommand::Bell(3));
    piano.insert(Keycode::E, AudioCommand::Bell(4));
    piano.insert(Keycode::R, AudioCommand::Bell(5));
    piano.insert(Keycode::Key5, AudioCommand::Bell(6));
    piano.insert(Keycode::T, AudioCommand::Bell(7));
    piano.insert(Keycode::Key6, AudioCommand::Bell(8));
    piano.insert(Keycode::Y, AudioCommand::Bell(9));
    piano.insert(Keycode::Key7, AudioCommand::Bell(10));
    piano.insert(Keycode::U, AudioCommand::Bell(11));
    piano.insert(Keycode::I, AudioCommand::Bell(12));

    let throttle_ms = 50;
    let mut last_sent_slider = time::SystemTime::now();
    let mut current_effect:usize = 0;
    let mut current_param:usize = 0;
    let effect_bank = [&effect::DELAY, &effect::HARDCLIP];
    let mut current_to_add:usize = 0;
    
    let mut mode = Modes::PERFORM;

    let mut effect_stack:Vec<&'static effect::EffectDefinition> = Vec::new();
    let mut effect_params:Vec<Vec<f32>> = Vec::new();
    let mut effect_wetness:Vec<f32> = Vec::new();

    let redraw = |
        mode: &Modes,
        effect_stack: &Vec<&'static effect::EffectDefinition>,
        effect_wetness: &Vec<f32>,
        effect_params: &Vec<Vec<f32>>,
        effect_bank: &[&effect::EffectDefinition; 2],
        current_effect: &usize,
        current_param: &usize,
        current_to_add: &usize,
    | {
        print!("\n\n\n");
        println!("TAB switch mode | h ← | k ↑ | j | ↓ l → | mouseX effect slider(hug the top side)");
        match mode {
            Modes::PERFORM => {
                println!("PERFORM MODE");
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
                println!("current effects:");
                for i in 0..effect_stack.len() {
                    println!("{}", effect_stack[i].title);
                }
                println!("");
                println!("press enter to add:");
                for i in 0..effect_bank.len() {
                    let hovering = i==*current_to_add;
                    println!("{}\t{}",
                             if hovering {">"} else {" "},
                             effect_bank[i].title
                    );
                }
            },
        };
    };

    redraw(
        &mode,
        &effect_stack,
        &effect_wetness,
        &effect_params,
        &effect_bank,
        &current_effect,
        &current_param,
        &current_to_add,
    );

    let add_effect = |
        prod: &mut Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
        effects: &mut Vec<&'static effect::EffectDefinition>,
        wetness: &mut Vec<f32>,
        params: &mut Vec<Vec<f32>>,
        to_add: &'static effect::EffectDefinition,
    | {
        effects.push(to_add);
        wetness.push(1.0);
        let default_values = (to_add.init)().params;
        let mut vec = Vec::new();
        for i in 0..to_add.param_count { vec.push(default_values[i]); }
        params.push(vec);
        let _ = prod.try_push(AudioCommand::NewEffect(to_add));
    };

    let set_param = |
        prod: &mut Caching<Arc<SharedRb<Heap<AudioCommand>>>, true, false>,
        params: &mut Vec<Vec<f32>>,
        wetness: &mut Vec<f32>,
        which_effect: usize,
        which_param: usize,
        to_value: f32,
    | {
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

        for (k,v) in &piano {
            if just_pressed.contains(&k) {
                let _ = prod.try_push(v.clone());
            }
        }

        match mode {
            Modes::PERFORM => {
                if just_pressed.contains(&Keycode::K) {
                    current_effect = current_effect.saturating_sub(1);
                }
                if just_pressed.contains(&Keycode::J) {
                    current_effect = usize::min(current_effect+1, effect_params.len());
                }
                if just_pressed.contains(&Keycode::H) {
                    current_param = current_param.saturating_sub(1);
                }
                if just_pressed.contains(&Keycode::L) {
                    current_param = usize::min(current_param+1, effect_params[current_effect].len()+1);
                }
                
                if last_sent_slider.elapsed().is_ok_and(|dur| dur.as_millis()>throttle_ms) {
                    let mouse = device_reader.get_mouse().coords;
                    // only change values when it's on the top side
                    if mouse.1<120 {
                        let slider = (mouse.0 as f32) / 1920.0;
                        set_param(
                            &mut prod,
                            &mut effect_params,
                            &mut effect_wetness,
                            current_effect,
                            current_param,
                            slider,
                        );
                        redraw(
                            &mode,
                            &effect_stack,
                            &effect_wetness,
                            &effect_params,
                            &effect_bank,
                            &current_effect,
                            &current_param,
                            &current_to_add,
                        );
                        last_sent_slider = time::SystemTime::now();
                    }
                }

                if just_pressed.contains(&Keycode::Tab) {
                    mode = Modes::EDIT;
                }
            },
            Modes::EDIT => {
                if just_pressed.contains(&Keycode::K) {
                    current_to_add = current_to_add.saturating_sub(1);
                }
                if just_pressed.contains(&Keycode::J) {
                    current_to_add = usize::min(current_to_add+1, effect_bank.len());
                }

                if just_pressed.contains(&Keycode::Enter) {
                    add_effect(
                        &mut prod,
                        &mut effect_stack,
                        &mut effect_wetness,
                        &mut effect_params,
                        effect_bank[current_to_add]
                    );
                }

                if just_pressed.contains(&Keycode::Tab) {
                    mode = Modes::PERFORM;
                }
            },
        };

        // stuff was entered to the terminal, redraw
        if !just_pressed.is_empty() {
            redraw(
                &mode,
                &effect_stack,
                &effect_wetness,
                &effect_params,
                &effect_bank,
                &current_effect,
                &current_param,
                &current_to_add,
            );
        }

        if keys.contains(&Keycode::Escape) {
            break;
        }

        pressed = keys;
    }

    Ok(())
}
