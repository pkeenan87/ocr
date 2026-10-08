use std::process::ExitCode;

use clap::Parser;

use ocr::cli::{Cli, Command, Engine, ScanArgs, ServerCommand, SetupArgs, StartArgs};
use ocr::config::Config;
use ocr::engines::MLX_VLM_PIN;
use ocr::error::CliError;
use ocr::input::{self, Plan};
use ocr::model_engine;
use ocr::pipeline::{self, Settings};
use ocr::server::{self, ServerInfo};
use ocr::vision::Options;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(e.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Server { command } => match command {
            ServerCommand::Setup(args) => setup(&args),
            ServerCommand::Start(args) => start(&args),
            ServerCommand::Stop => server::stop(),
            ServerCommand::Status => server::status(),
        },
        Command::Scan(args) => scan(&args),
    }
}

/// Setup gate (PLAN §3): warns, and fails unless the engine is usable.
fn gate(engine: Engine) -> Result<(), CliError> {
    match Config::load()? {
        None => {
            let warning = "⚠ ocr is not set up yet. Run `ocr server setup` first.";
            if engine == Engine::Apple {
                eprintln!("{warning}");
                Ok(())
            } else {
                Err(CliError::Setup(warning.into()))
            }
        }
        Some(cfg) if cfg.is_installed(engine) => Ok(()),
        Some(_) => {
            let n = engine.name();
            Err(CliError::Setup(format!(
                "⚠ Engine '{n}' is not installed. Run `ocr server setup --model {n}`."
            )))
        }
    }
}

fn setup(args: &SetupArgs) -> Result<(), CliError> {
    let mut cfg = Config::load()?.unwrap_or_default();
    cfg.mark_apple_installed();
    cfg.save()?;
    std::fs::create_dir_all(ocr::config::state_dir()?).map_err(anyhow::Error::from)?;
    println!("apple engine ready (no download needed)");

    server::setup_models(args, &mut cfg)
}

fn start(args: &StartArgs) -> Result<(), CliError> {
    let model = match args.model {
        Some(Engine::Apple) => {
            return Err(CliError::Usage(
                "the apple engine runs in-process and needs no server".into(),
            ));
        }
        Some(m) => m,
        None => default_server_model()?,
    };
    gate(model)?;
    server::start(args, model)
}

/// With no `--model`, serve the one installed model engine; otherwise ask the user to choose.
fn default_server_model() -> Result<Engine, CliError> {
    let config = Config::load().map_err(|e| CliError::Setup(e.to_string()))?;
    let installed: Vec<Engine> = config
        .map(|c| {
            Engine::MODELS
                .into_iter()
                .filter(|m| c.is_installed(*m))
                .collect()
        })
        .unwrap_or_default();
    match installed.as_slice() {
        [only] => Ok(*only),
        [] => Err(CliError::Setup(
            "no model engine is installed. Run `ocr server setup --model <engine>`.".into(),
        )),
        many => Err(CliError::Usage(format!(
            "several model engines are installed; choose one with --model ({})",
            many.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

fn scan(args: &ScanArgs) -> Result<(), CliError> {
    args.model.resolve_version(args.model_version.as_deref())?;
    gate(args.model)?;

    let plan = match (&args.input_file, &args.input_dir) {
        (Some(f), _) => input::plan_single(
            f,
            args.output_file.as_deref(),
            args.file_type,
            args.overwrite,
        )?,
        (None, Some(d)) => input::plan_dir(
            d,
            args.output_dir.as_deref(),
            args.recursive,
            args.file_type,
            args.overwrite,
        )?,
        (None, None) => unreachable!("clap requires an input"),
    };
    for note in &plan.notes {
        eprintln!("{note}");
    }
    if args.model != Engine::Apple && args.jobs.is_some_and(|j| j > 1) {
        eprintln!("note: --jobs > 1 with a model engine; output may not be byte-reproducible");
    }
    if plan.skipped_existing > 0 {
        eprintln!(
            "skipped {} file(s) with existing output (use --overwrite)",
            plan.skipped_existing
        );
    }
    if plan.items.is_empty() {
        eprintln!("⚠ no matching files to process");
        return Ok(());
    }

    if args.dry_run {
        println!(
            "Dry run: {} file(s), jobs {}",
            plan.items.len(),
            args.effective_jobs()
        );
        print_mapping(&plan);
        return Ok(());
    }
    if args.model != Engine::Apple {
        let info = model_gate(args)?;
        return run_model(args, &plan, info);
    }
    run_apple(args, &plan)
}

/// Model-engine scan gate (PLAN §3): the right server must be running.
fn model_gate(args: &ScanArgs) -> Result<ServerInfo, CliError> {
    let name = args.model.name();
    let Some(info) = server::running() else {
        return Err(CliError::Server(format!(
            "model server not running; run 'ocr server start --model {name}'"
        )));
    };
    let (_, _, repo) = server::resolve_model(args.model, args.model_version.as_deref(), None)?;
    if info.engine != name || info.repo != repo {
        return Err(CliError::Server(format!(
            "running server is {} ({}), but the scan needs {name} ({repo}); \
             run 'ocr server start --model {name} --force'",
            info.engine, info.repo
        )));
    }
    Ok(info)
}

/// Settings shared by every engine.
fn settings(args: &ScanArgs) -> Settings {
    Settings {
        dpi: args.effective_dpi(),
        jobs: args.effective_jobs(),
        pages: args.pages.clone(),
        text_layer: args.effective_text_layer(),
        page_markers: args.page_markers(),
        front_matter: !args.no_front_matter,
        currency_fix: !args.no_currency_fix,
        quiet: args.quiet,
        verbose: args.verbose,
        options: Options::default(),
    }
}

/// Runs the PaddleOCR-VL engine over every planned file. Pages that fell back to Apple Vision
/// or failed make the run partial; a stopped server ends it with exit 3.
fn run_model(args: &ScanArgs, plan: &Plan, info: ServerInfo) -> Result<(), CliError> {
    let mlx = Config::load()?
        .and_then(|c| c.runtime)
        .map_or_else(|| MLX_VLM_PIN.to_string(), |r| r.mlx_vlm);
    let spec = model_engine::scan_engine(args.model).expect("model engines have a scan engine");
    if args.model == Engine::Olmocr && args.dpi.is_some() && !args.quiet {
        eprintln!(
            "note: --dpi is ignored for olmocr; images are always 1288 px on the longest side"
        );
    }
    let ctx = model_engine::Ctx::new(spec, info, &mlx);
    let settings = settings(args);
    let mut problems = Vec::new();
    for item in &plan.items {
        match model_engine::run_file(item, &settings, &ctx) {
            Ok(report) if report.flagged.is_empty() => {}
            Ok(report) => problems.push(format!(
                "{}: {} of {} pages flagged (pages {:?}); see fallback_pages in the front matter",
                item.input.display(),
                report.flagged.len(),
                report.pages,
                report.flagged
            )),
            Err(model_engine::RunError::Stopped(msg)) => {
                return Err(CliError::Server(format!("{msg}; re-run to start over")));
            }
            Err(model_engine::RunError::Other(e)) => {
                problems.push(format!("{}: {e:#}", item.input.display()));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CliError::Partial(format!(
            "completed with problems:\n  {}",
            problems.join("\n  ")
        )))
    }
}

/// Runs the apple pipeline over every planned file; failed pages or files make the run partial.
fn run_apple(args: &ScanArgs, plan: &Plan) -> Result<(), CliError> {
    let settings = settings(args);
    let mut problems = Vec::new();
    for item in &plan.items {
        match pipeline::run_file(item, &settings) {
            Ok(report) if report.flagged.is_empty() => {}
            Ok(report) => problems.push(format!(
                "{}: {} of {} pages failed OCR (pages {:?})",
                item.input.display(),
                report.flagged.len(),
                report.pages,
                report.flagged
            )),
            Err(e) => problems.push(format!("{}: {e:#}", item.input.display())),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CliError::Partial(format!(
            "completed with problems:\n  {}",
            problems.join("\n  ")
        )))
    }
}

fn print_mapping(plan: &Plan) {
    for item in &plan.items {
        println!("{} -> {}", item.input.display(), item.output.display());
    }
}
