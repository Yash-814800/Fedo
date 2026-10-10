// fedo-stt — speech to text, one utterance per run.
//
// Kept out of the daemon on purpose: whisper.cpp and its model are hundreds of
// megabytes, and Fedo's idle budget is 40 MB. The daemon starts this only when
// the user dictates, and the memory goes back when it exits.
//
//   fedo-stt --model <ggml-*.bin> [--lang auto|en|hi|…] [--threads N] < pcm.s16le
//
// stdin: raw signed 16-bit little-endian mono PCM at 16 kHz.
// stdout: the transcript, one line. Exit code 0 even for silence (empty line).

use std::io::{Read, Write};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let mut model = None;
    let mut lang = "auto".to_string();
    let mut threads = 4;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model" => model = args.next(),
            "--lang" => lang = args.next().unwrap_or(lang),
            "--threads" => threads = args.next().and_then(|n| n.parse().ok()).unwrap_or(threads),
            _ => {}
        }
    }
    let Some(model) = model else {
        eprintln!("usage: fedo-stt --model <ggml.bin> [--lang auto] < pcm.s16le");
        std::process::exit(2);
    };

    let mut bytes = Vec::new();
    if std::io::stdin().read_to_end(&mut bytes).is_err() {
        std::process::exit(1);
    }
    let audio: Vec<f32> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
    // Under a third of a second is a click, not speech; whisper hallucinates on it.
    if audio.len() < 16_000 / 3 {
        println!();
        return;
    }

    let ctx = match WhisperContext::new_with_params(&model, WhisperContextParameters::default()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot load {model}: {e}");
            std::process::exit(1);
        }
    };
    let mut state = ctx.create_state().expect("whisper state");
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads);
    params.set_language(Some(&lang));
    params.set_translate(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_no_context(true);
    if let Err(e) = state.full(params, &audio) {
        eprintln!("transcription failed: {e}");
        std::process::exit(1);
    }
    let mut text = String::new();
    for segment in state.as_iter() {
        text.push_str(&segment.to_string());
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Whisper writes silence and noise as "[BLANK_AUDIO]", "[MUSIC]", "(silence)"…
    let text = if text.starts_with(['[', '(']) && text.ends_with([']', ')']) && !text[1..].contains(['[', '(']) {
        String::new()
    } else {
        text
    };
    let _ = writeln!(std::io::stdout(), "{text}");
}
