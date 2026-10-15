use clap::{Args, Parser, Subcommand};
use retarjet::classify;
use retarjet::commands::{self, Mode, PoseSource};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "Retarget animations between GLB files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Retarget animations from one GLB onto another
    Retarget(RetargetArguments),
    /// Write a single frame clip to use as a reference pose
    Pose {
        file: PathBuf,
        /// Name of the clip to write
        #[arg(long, default_value = "t-pose")]
        name: String,
        /// Pose to capture
        #[arg(long, value_enum, default_value_t = PoseSource::Bind)]
        from: PoseSource,
        /// Replace the clip if it already exists
        #[arg(long)]
        overwrite: bool,
        /// Write the result here instead of updating the file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Classify the rig with a local model and cache the result
    Classify {
        file: PathBuf,
        /// Classify again even if the rig is already cached
        #[arg(long)]
        force: bool,
        /// Ollama model to classify with
        #[arg(long, env = "RETARJET_MODEL", default_value = classify::DEFAULT_MODEL)]
        model: String,
    },
    /// Show the skeleton, classification and clips of a GLB
    Info { file: PathBuf },
}

#[derive(Args)]
struct RetargetArguments {
    /// GLB to take animations from
    source: PathBuf,
    /// GLB to retarget the animations onto
    destination: PathBuf,
    /// Reference pose clip, one name for both files or one per file [default: each file's default pose]
    #[arg(long, num_args = 1..=2, value_names = ["SOURCE", "DESTINATION"])]
    pose: Vec<String>,
    /// Facing axis (+x, -x, +z or -z), one for both files or one per file
    #[arg(
        long,
        num_args = 1..=2,
        value_names = ["SOURCE", "DESTINATION"],
        default_value = "+z",
        allow_hyphen_values = true,
        value_parser = ["+x", "-x", "+z", "-z"],
    )]
    facing: Vec<String>,
    /// What happens to the animations the destination already has
    #[arg(long, value_enum, default_value_t)]
    mode: Mode,
    /// Write the result here instead of updating the destination
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Ollama model that classifies rigs without a cached classification
    #[arg(long, env = "RETARJET_MODEL", default_value = classify::DEFAULT_MODEL)]
    model: String,
}

fn retarget(arguments: RetargetArguments) -> anyhow::Result<()> {
    let options = commands::RetargetOptions {
        poses: &arguments.pose,
        facings: &arguments.facing,
        mode: arguments.mode,
        output: arguments.output.as_deref(),
        model: &arguments.model,
    };
    commands::retarget(&arguments.source, &arguments.destination, &options)
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Retarget(arguments) => retarget(arguments),
        Command::Pose {
            file,
            name,
            from,
            overwrite,
            output,
        } => commands::pose(&file, &name, from, overwrite, output.as_deref()),
        Command::Classify { file, force, model } => commands::classify(&file, force, &model),
        Command::Info { file } => commands::info(&file),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
