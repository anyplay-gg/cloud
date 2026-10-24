use anyhow::Result;
use anyplay_load::{Options, run};
use clap::Parser as _;

fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let opts = Options::parse();
    opts.validate()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(opts.threads.max(1))
        .thread_name("harness")
        .enable_all()
        .build()?;
    let report = runtime.block_on(run(opts))?;
    println!(
        "{} on {} ({} cores, {} GB), store {}, {} node(s), commit {}",
        report.date,
        report.machine.cpu,
        report.machine.logical_cores,
        report.machine.memory_gb,
        report.store,
        report.nodes,
        report.commit.as_deref().unwrap_or("unknown")
    );
    // The in-process nodes keep threads of their own; do not wait for them.
    std::process::exit(0);
}
