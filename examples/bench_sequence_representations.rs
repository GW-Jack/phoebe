//! Isolated host-side collection layout experiment, not a Phoebe runtime.
//! Run: rustc --edition=2021 -O examples/bench_sequence_representations.rs -o /tmp/phoebe-sequences
//!      /tmp/phoebe-sequences 100000 1000 9

use std::hint::black_box;
use std::time::{Duration, Instant};

const CHUNK: usize = 256;

trait Sequence {
    fn with_limit(limit: usize) -> Self;
    fn push(&mut self, value: u64);
    fn get(&self, index: usize) -> u64;
    fn sum(&self) -> u64;
}

struct Contiguous(Vec<u64>);

impl Sequence for Contiguous {
    fn with_limit(limit: usize) -> Self {
        Self(Vec::with_capacity(limit))
    }
    fn push(&mut self, value: u64) {
        self.0.push(value);
    }
    fn get(&self, index: usize) -> u64 {
        self.0[index]
    }
    fn sum(&self) -> u64 {
        self.0.iter().copied().sum()
    }
}

struct Segmented(Vec<Vec<u64>>);

impl Sequence for Segmented {
    fn with_limit(limit: usize) -> Self {
        Self(Vec::with_capacity(limit.div_ceil(CHUNK)))
    }
    fn push(&mut self, value: u64) {
        if self.0.last().is_none_or(|chunk| chunk.len() == CHUNK) {
            self.0.push(Vec::with_capacity(CHUNK));
        }
        self.0.last_mut().unwrap().push(value);
    }
    fn get(&self, index: usize) -> u64 {
        self.0[index / CHUNK][index % CHUNK]
    }
    fn sum(&self) -> u64 {
        self.0.iter().flat_map(|chunk| chunk.iter()).copied().sum()
    }
}

struct Node {
    value: u64,
    next: Option<usize>,
}

struct RegionLinked {
    nodes: Vec<Node>,
    first: Option<usize>,
    last: Option<usize>,
}

impl Sequence for RegionLinked {
    fn with_limit(limit: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(limit),
            first: None,
            last: None,
        }
    }
    fn push(&mut self, value: u64) {
        // This prototype knows its maximum size and never reuses a node.
        // The entire node region is released when the sequence is dropped.
        assert!(self.nodes.len() < self.nodes.capacity());
        let index = self.nodes.len();
        self.nodes.push(Node { value, next: None });
        if let Some(last) = self.last {
            self.nodes[last].next = Some(index);
        } else {
            self.first = Some(index);
        }
        self.last = Some(index);
    }
    fn get(&self, index: usize) -> u64 {
        let mut node = self.first.expect("nonempty sequence");
        for _ in 0..index {
            node = self.nodes[node].next.expect("index in bounds");
        }
        self.nodes[node].value
    }
    fn sum(&self) -> u64 {
        let mut node = self.first;
        let mut total = 0;
        while let Some(index) = node {
            total += self.nodes[index].value;
            node = self.nodes[index].next;
        }
        total
    }
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort_unstable();
    times[times.len() / 2]
}

fn measure<S: Sequence>(name: &str, count: usize, reads: usize, rounds: usize) {
    let expected = (count as u64) * ((count - 1) as u64) / 2;
    let expected_reads = (0..reads)
        .map(|read| (read.wrapping_mul(65_537) % count) as u64)
        .fold(0u64, u64::wrapping_add);
    let mut build_scan = Vec::new();
    let mut indexed = Vec::new();
    for _ in 0..rounds {
        let start = Instant::now();
        let mut sequence = S::with_limit(count);
        for index in 0..count {
            sequence.push(black_box(index as u64));
        }
        assert_eq!(black_box(sequence.sum()), expected);
        build_scan.push(start.elapsed());

        let start = Instant::now();
        let mut checksum = 0u64;
        for read in 0..reads {
            let index = read.wrapping_mul(65_537) % count;
            checksum = checksum.wrapping_add(black_box(sequence.get(black_box(index))));
        }
        assert_eq!(checksum, expected_reads);
        black_box(checksum);
        indexed.push(start.elapsed());
    }
    println!(
        "{name:14} build+scan {:9.3} ms   indexed reads {:9.3} ms",
        median(build_scan).as_secs_f64() * 1000.0,
        median(indexed).as_secs_f64() * 1000.0,
    );
}

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    let parse = |index: usize, default: usize| {
        arguments
            .get(index)
            .map_or(default, |value| value.parse().expect("positive integer"))
    };
    let count = parse(1, 100_000);
    let reads = parse(2, 1_000);
    let rounds = parse(3, 9);
    assert!(count > 0 && reads > 0 && rounds > 0);
    println!("count={count} indexed_reads={reads} rounds={rounds} chunk={CHUNK}");
    measure::<Contiguous>("contiguous", count, reads, rounds);
    measure::<Segmented>("segmented", count, reads, rounds);
    measure::<RegionLinked>("region linked", count, reads, rounds);
}
