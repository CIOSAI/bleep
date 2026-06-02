// with much help from https://whoisryosuke.com/blog/2026/creating-a-daw-in-rust/

use core::f32;
use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SizedSample, Sample, I24
};
use ringbuf::{
    HeapRb, SharedRb, traits::{Producer, Consumer, Split}, wrap::caching::{Caching}, storage::{Heap}, 
};
use device_query::{DeviceQuery, DeviceState, Keycode};

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
#[derive(Clone)]
enum AudioCommand {
    Bell(i8),
    DelayVolume(f32),
    Hardclip(f32, f32),
}

const DELAY_BUFFER_SIZE:usize = 1<<12;

pub struct Player {
    cached_sounds: HashMap<i8, Arc<Vec<f32>>>,
    playing: Vec<OneSound>,
    consumer: Caching<Arc<SharedRb<Heap<AudioCommand>>>, false, true>,
    hardclip_amount: f32,
    hardclip_postgain: f32,
    delay_volume: f32,
    delay_buffer: [f32; DELAY_BUFFER_SIZE],
    delay_pointer: usize,
}
impl Player {
    //                           at least 1
    //let conv = |x:f32,center:f32,spread:f32| f32::exp(-(x-center).powi(2)/spread.powi(2))/spread;
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
                                val = if (t*pitch).fract()<0.5 { -1.0 } else { 1.0 };
                                val *= f32::exp(-t.fract() * decay) * volume;
                            }

                            buffer.push(val);
                        }

                        self.cached_sounds.insert(note_name.clone(), Arc::new(buffer));
                    }
                },
                AudioCommand::DelayVolume(volume) => {
                    self.delay_volume = volume;
                },
                AudioCommand::Hardclip(amount, postgain) => {
                    self.hardclip_amount = amount;
                    self.hardclip_postgain = postgain;
                },
            }
        }
        for frame in data.chunks_mut(channels as usize) {
            for sample in 0..channels {
                let mut accum:f32 = 0.0;
                self.playing.retain_mut(|sound| {
                    match self.cached_sounds.get(&sound.note_pitch) {
                        Some(buffer) => {
                            if sound.position>=buffer.len() {
                                false
                            }
                            else {
                                // refactor this
                                let num = buffer.get(sound.position).unwrap();
                                sound.position += 1;
                                accum += num;
                                true
                            }
                        },
                        None => { false }
                    }
                });

                accum = (accum*self.hardclip_amount).clamp(-1.0, 1.0)*self.hardclip_postgain;

                let delay_sound = self.delay_buffer[(self.delay_pointer+1)%DELAY_BUFFER_SIZE];

                accum += delay_sound*self.delay_volume;
                accum = accum.clamp(-1.0, 1.0);

                self.delay_buffer[self.delay_pointer] = accum;
                self.delay_pointer = (self.delay_pointer+1)%DELAY_BUFFER_SIZE;

                frame[sample as usize] = Sample::from_sample(accum);
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
        // 32 slots for audio commands (tell audio engine what pitch to play)
        let ring = HeapRb::<AudioCommand>::new(32);
        let (producer, consumer) = ring.split();

        Self {
            producer: producer,
            player: Player {
                cached_sounds: HashMap::new(),
                playing: Vec::new(),
                consumer: consumer,
                hardclip_amount: 1.0,
                hardclip_postgain: 1.0,
                delay_volume: 0.0,
                delay_buffer: [0.0; DELAY_BUFFER_SIZE],
                delay_pointer: 0,
            }
        }
    }
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

    let keyboard_reader = DeviceState::new();
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

    piano.insert(Keycode::A, AudioCommand::DelayVolume(0.5));
    piano.insert(Keycode::S, AudioCommand::DelayVolume(0.0));

    piano.insert(Keycode::D, AudioCommand::Hardclip(32.0, 0.1));
    piano.insert(Keycode::F, AudioCommand::Hardclip(1.0, 1.0));

    loop {
        let keys = keyboard_reader.get_keys();

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

        if keys.contains(&Keycode::Escape) {
            break;
        }

        pressed = keys;
    }

    Ok(())
}
