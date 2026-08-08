fn main() {
    use boris_inference::TextToSpeech;
    use boris_tts_supertone::SupertoneTts;
    let home = std::env::var("USERPROFILE").unwrap();
    let onnx = format!(r"{}\.boris\models\supertone\onnx", home);
    let voices = format!(r"{}\.boris\models\supertone\voices", home);
    let mut tts = SupertoneTts::with_paths(&onnx, &voices, "M4");
    tts.load().expect("load");
    let text = "Phone stuff is easy. I am basically a genius.";
    let pcm = tts.synthesize(text).expect("synth");
    println!("text={text:?}");
    println!("samples={} duration_secs={:.2}", pcm.len(), pcm.len() as f32 / 44100.0);
    assert!(pcm.len() > 20_000, "audio too short: {}", pcm.len());
    // Peak energy check — silence would fail
    let peak = pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    println!("peak_abs={peak:.4}");
    assert!(peak > 0.01, "audio nearly silent");
    println!("OK");
}
