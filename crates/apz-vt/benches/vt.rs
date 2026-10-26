//! Throughput of the VT emulator: bytes fed through `Emulator::feed` into a 120x40 screen.
//!
//! Each workload is one prebuilt buffer of at least 1 MiB, made of whole units (a line, or a
//! block of lines) so that no escape sequence or wide character is cut. The buffer is built
//! once, outside the timed section, and each iteration feeds it into a fresh emulator built
//! outside the timed section too, so the numbers are parsing and screen updates only.

use std::hint::black_box;

use apz_vt::Emulator;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};

const MIB: usize = 1024 * 1024;
const COLS: usize = 120;
const ROWS: usize = 40;

/// Colour, style and cursor traffic: 16, 256 and truecolour SGR, bold, underline and inverse,
/// cursor positioning (CUP), erase line (EL) and resets, every few words.
const COLOURED_UNIT: &str = concat!(
    "\x1b[1;31mERROR\x1b[0m disk \x1b[38;5;208mwarn\x1b[0m on ",
    "\x1b[38;2;10;20;30mnode-7\x1b[0m, retrying in 5s for job build-42\r\n",
    "\x1b[10;5H\x1b[2K\x1b[32mok\x1b[0m cache hit, \x1b[1mbold\x1b[22m and ",
    "\x1b[4munderline\x1b[24m text\r\n",
    "\x1b[7minverse\x1b[27m \x1b[48;5;236mdim bg\x1b[49m ",
    "\x1b[38;2;200;120;40mamber\x1b[0m words in one more line\r\n",
);

/// Wide text: CJK ideographs and kana (two cells each), and emoji, mixed with ASCII.
const WIDE_UNIT: &str = concat!(
    "漢字かな交じり文 mixed with ASCII: 漢字 漢字 漢字 中文字符测试\r\n",
    "ロケット 🚀 deploy 完了 ✅ 한국어 텍스트 and 🎉 party 🎉\r\n",
    "漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字\r\n",
    "plain ascii line, then 漢字 🚀\r\n",
);

/// Repeats `unit` until the buffer holds at least a MiB. The result is a little over a MiB
/// when the unit does not divide it evenly, and the throughput uses the real length.
fn repeat_to_mib(unit: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MIB + unit.len());
    while out.len() < MIB {
        out.extend_from_slice(unit);
    }
    out
}

/// Plain ASCII: 80 printable characters and a CRLF per line, so the screen scrolls constantly.
fn plain_ascii() -> Vec<u8> {
    let line: Vec<u8> = (0..80u8)
        .map(|i| b'!' + i)
        .chain(b"\r\n".iter().copied())
        .collect();
    repeat_to_mib(&line)
}

fn coloured() -> Vec<u8> {
    repeat_to_mib(COLOURED_UNIT.as_bytes())
}

fn wide_utf8() -> Vec<u8> {
    repeat_to_mib(WIDE_UNIT.as_bytes())
}

fn bench_feed(c: &mut Criterion) {
    let workloads = [
        ("plain_ascii_scroll", plain_ascii()),
        ("sgr_cursor_erase", coloured()),
        ("wide_utf8", wide_utf8()),
    ];

    let mut group = c.benchmark_group("feed_120x40");
    for (name, data) in &workloads {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_function(*name, |b| {
            // PerIteration: one emulator alive at a time. A full scrollback holds 10 000 lines,
            // so batching several inputs ahead would multiply the memory use.
            b.iter_batched_ref(
                || Emulator::new(COLS, ROWS),
                |emu| {
                    emu.feed(black_box(data.as_slice()));
                    black_box(emu.screen());
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_feed);
criterion_main!(benches);
