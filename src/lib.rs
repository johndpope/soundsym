extern crate voting_experts;
extern crate vox_box;
extern crate hound;
extern crate cogset;

use vox_box::spectrum::MFCC;
use vox_box::waves::{WindowType, Windower, Filter};
use cogset::{Euclid, Kmeans, KmeansBuilder};
use voting_experts::{cast_votes, split_string};

use std::path::Path;
use std::error::Error;
use std::str::from_utf8;
use std::cmp::Ordering;

pub const NCOEFFS: usize = 12;
pub const HOP: usize = 2048;

pub fn calc_mfccs(path: &Path) -> Vec<Euclid<[f64; NCOEFFS]>> {
    let mut file = hound::WavReader::open(path).unwrap();
    let sample_rate = file.spec().sample_rate;
    let mut samples: Vec<f64> = file.samples::<i32>().map(|s| s.unwrap() as f64).collect();
    samples.preemphasis(50f64 / sample_rate as f64);

    let sample_windows = Windower::new(WindowType::Hanning, &samples[..], HOP, HOP);
    let mfcc_calc = |frame: Vec<f64>| -> Euclid<[f64; NCOEFFS]> { 
        let mut mfccs = [0f64; NCOEFFS];
        let m = frame.mfcc(NCOEFFS, (100., 8000.), sample_rate as f64);
        for (i, c) in m.iter().enumerate() {
            mfccs[i] = *c;
        }
        Euclid(mfccs)
    };
    sample_windows.map(&mfcc_calc).collect()
}

pub fn discretize(data: &[Euclid<[f64; NCOEFFS]>]) -> Kmeans<[f64; NCOEFFS]> {
    let k = 50;
    let tol = 1e-10;
    KmeansBuilder::new().tolerance(tol).kmeans(data, k)
}

pub fn partition<'a>(path: &'a Path) -> Result<(&'a Path, Vec<usize>), Box<Error>> {
    let mfccs = calc_mfccs(path);
    let clusters = discretize(&mfccs[..]).clusters();

    // Symbol to start the gibberish from
    let start_symbol = "A".as_bytes()[0];
    // Initialize memory for a u8 vector with one element per mfcc frame
    let mut byte_string = vec![0u8; mfccs.len()];
    // Look up the frame of each element in each cluster, and assign to it that cluster's label.
    for (idx, cluster) in clusters.iter().enumerate() {
        let value = idx as u8 + start_symbol;
        for &index in cluster.1.iter() {
            byte_string[index] = value as u8;
        }
    }

    let text_string = try!(from_utf8(&byte_string[..]));
    let votes = cast_votes(&text_string, 4);
    let splits = split_string(&text_string, &votes, 4, 3);
    let segment_lengths = splits.into_iter().map(|s| s.len() * HOP).collect();
    Ok((path, segment_lengths))
}

/// Takes the path of a source file, and a series of sample lengths, and splits the file
/// accordingly into a bunch of short files
pub fn write_splits(path: &Path, splits: &Vec<usize>) -> Result<(), Box<Error>> {
    let mut file = try!(hound::WavReader::open(path));
    let sample_rate = file.spec().sample_rate;
    let mut samples = file.samples::<i32>().map(|s| s.unwrap());

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate,
        bits_per_sample: 24
    };

    // Works through the samples in order, writing files to disk
    for (idx, split) in splits.iter().enumerate() {
        let mut writer = try!(hound::WavWriter::create(format!("data/output/{:02}_{}.wav", idx, split), spec));
        for sample in samples.by_ref().take(*split) {
            try!(writer.write_sample(sample));
        }
        try!(writer.finalize());
    }

    Ok(())
}

pub struct SoundDictionary {
    sounds: Vec<Sound>
}

pub struct Sound {
    max_power: f64,
    samples: Vec<f64>,
    mfccs: Vec<Euclid<[f64; NCOEFFS]>>
}

impl SoundDictionary {
    pub fn from_path(path: &Path) -> Result<SoundDictionary, Box<Error>> {
        let dir = try!(path.read_dir());
        let mut out = SoundDictionary { 
            sounds: Vec::<Sound>::with_capacity(dir.size_hint().0) 
        };

        for res in dir {
            let entry = try!(res);
            let mut file = try!(hound::WavReader::open(entry.path()));
            let samples: Vec<f64> = file.samples::<i32>().map(|s| s.unwrap_or(0) as f64).collect();
            let sample_rate = file.spec().sample_rate;
            out.sounds.push(Sound::from_samples(samples, sample_rate as f64));
        }
        Ok(out)
    }
}

impl Sound {
    pub fn from_samples(old_samples: Vec<f64>, sample_rate: f64) -> Sound {
        let mut samples = old_samples.clone();
        samples.preemphasis(50f64 / sample_rate);

        let sample_windows = Windower::new(WindowType::Hanning, &samples[..], HOP, HOP);
        let mfcc_calc = |frame: Vec<f64>| -> Euclid<[f64; NCOEFFS]> { 
            let mut mfccs = [0f64; NCOEFFS];
            let m = frame.mfcc(NCOEFFS, (100., 8000.), sample_rate as f64);
            for (i, c) in m.iter().enumerate() {
                mfccs[i] = *c;
            }
            Euclid(mfccs)
        };

        let power_calc = |frame: Vec<f64>| -> f64 {
            let len = frame.len() as f64;
            (frame.iter().fold(0., |acc, s| acc + s.powi(2)) / len).sqrt()
        };

        let mfccs = sample_windows.map(&mfcc_calc).collect();
        let mut max_power = 0.;

        {
            let power_windows = Windower::new(WindowType::Rectangle, &old_samples[..], HOP, HOP);
            let mut powers: Vec<f64> = power_windows.map(&power_calc).collect();
            powers.sort_by(|a, b| b.partial_cmp(a).unwrap_or(Ordering::Equal));
            max_power = powers[0];
        }

        Sound { 
            max_power: max_power,
            samples: old_samples,
            mfccs: mfccs
        }
    }
}

#[test]
fn test_discretize() {
    let path = Path::new("data/sample.wav");
    let mfccs = calc_mfccs(path);
    let kmeans = discretize(&mfccs[..]);
    let clusters = kmeans.clusters();
}

#[test]
fn test_sound_from_samples() {
    let mut file = hound::WavReader::open(Path::new("data/sample.wav")).unwrap();
    let mut samples: Vec<f64> = file.samples::<i32>().map(|s| s.unwrap() as f64 / i32::max_value().wrapping_shr(8) as f64).collect();
    let sample_rate = file.spec().sample_rate;
    let sound = Sound::from_samples(samples.clone(), sample_rate as f64);
    samples.sort_by(|a, b| b.abs().partial_cmp(&a.abs()).unwrap_or(Ordering::Equal));
    println!("max i32: {}", i16::max_value());
    // println!("max val: {}", max_val);
    println!("max_power: {}", sound.max_power);
    println!("max sample: {}", samples[0]);
    assert!((samples[0] - 0.5961925502).abs() < 1e-9);
    assert!((sound.max_power - 0.19170839520179).abs() < 1e-12);
}
