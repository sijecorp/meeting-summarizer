// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `--plan-chunks` / `--transcribe-worker` (architecture 5.3), `--llm-supervisor` (6.6): the
    // same executable as a transcription worker or the summary engine's supervisor, which exits
    // inside. Before `run`, so a worker never opens a window or meets the single-instance check.
    meeting_summarizer_lib::worker_main();
    meeting_summarizer_lib::run()
}
