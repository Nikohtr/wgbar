//! Micro-benchmark of the logic WGBar runs on every poll, mirrored line for line by
//! bench/LogicBench.swift so the Swift and Rust numbers compare the same work.
//!
//! Usage: cargo run --release --example logic_bench -- bench/lsof-sample.txt tests/fixtures/sample.conf
//! Prints one `name ns_per_op` line per workload (median of 7 runs).

use std::hint::black_box;
use std::time::Instant;
use wgbar::dns_guard::config_dns_servers;
use wgbar::on_demand::{parse_allowed_ips, parse_lsof, vpn_bound};
use wgbar::tunnels::tunnel_address;

fn median_ns(iters: u32, mut f: impl FnMut() -> usize) -> f64 {
    let mut runs: Vec<f64> = (0..7)
        .map(|_| {
            let start = Instant::now();
            let mut sink = 0usize;
            for _ in 0..iters {
                sink = sink.wrapping_add(f());
            }
            black_box(sink);
            start.elapsed().as_nanos() as f64 / f64::from(iters)
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    runs[3]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let lsof = std::fs::read_to_string(&args[1]).expect("lsof sample");
    let conf = std::fs::read_to_string(&args[2]).expect("config sample");
    let allowed = parse_allowed_ips(&conf);
    let sockets = parse_lsof(&lsof);

    let parse = median_ns(2_000, || parse_lsof(black_box(&lsof)).len());
    let bound = median_ns(2_000, || vpn_bound(black_box(&sockets), &allowed).len());
    let tick = median_ns(2_000, || {
        vpn_bound(&parse_lsof(black_box(&lsof)), &allowed).len()
    });
    let config = median_ns(200_000, || {
        let c = black_box(&conf);
        config_dns_servers(c).len()
            + parse_allowed_ips(c).len()
            + tunnel_address(c).map_or(0, |a| a.len())
    });
    println!("parse_lsof_400 {parse:.0}");
    println!("vpn_bound_400 {bound:.0}");
    println!("poll_tick_400 {tick:.0}");
    println!("parse_config {config:.0}");
}
