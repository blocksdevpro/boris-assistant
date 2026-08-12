//! Manual smoke: synthesize a short phrase with installed Supertone weights.
//!
//! ```bash
//! cargo run -p boris-tts-supertone --example smoke_synth
//! ```
//!
//! Looks under `$HOME/.boris/models/supertone` (or `%USERPROFILE%` on Windows).

fn main() {
    use boris_inference::TextToSpeech;
    use boris_tts_supertone::SupertoneTts;
    use std::path::PathBuf;

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .expect("HOME or USERPROFILE must be set");
    let base = PathBuf::from(home)
        .join(".boris")
        .join("models")
        .join("supertone");
    let onnx = base.join("onnx");
    let voices = base.join("voices");

    println!("model_dir={}", onnx.display());
    println!("voice_dir={}", voices.display());

    let mut tts = SupertoneTts::with_paths(&onnx, &voices, "M4");
    tts.load().expect("load");
    let text = "Phone stuff is easy. I am basically a genius.";
    let pcm = tts.synthesize(text).expect("synth");
    let rate = tts.sample_rate().max(1) as f32;
    println!("text={text:?}");
    println!(
        "samples={} duration_secs={:.2} sample_rate={}",
        pcm.len(),
        pcm.len() as f32 / rate,
        tts.sample_rate()
    );
    assert!(pcm.len() > 20_000, "audio too short: {}", pcm.len());
    let peak = pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    println!("peak_abs={peak:.4}");
    assert!(peak > 0.01, "audio nearly silent");
    println!("OK");
}
