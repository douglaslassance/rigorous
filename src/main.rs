use clap::{Parser, Subcommand};
use retarjet::classify;
use retarjet::commands::{self, PoseSource};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "Combine and retarget animations between GLB files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Copy animations into a GLB that shares the same rig
    Combine {
        /// GLB to copy animations from
        source: PathBuf,
        /// GLB to add the animations to
        destination: PathBuf,
        /// Replace animations that already exist under the same name
        #[arg(long)]
        overwrite: bool,
        /// Write the result here instead of updating the destination
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Retarget animations onto a GLB with a different rig
    Transfer {
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
        /// Replace animations that already exist under the same name
        #[arg(long)]
        overwrite: bool,
        /// Write the result here instead of updating the destination
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Ollama model that classifies rigs without a stored classification
        #[arg(long, env = "RETARJET_MODEL", default_value = classify::DEFAULT_MODEL)]
        model: String,
    },
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
    /// Classify the rig with a local model and store the result in the file
    Classify {
        file: PathBuf,
        /// Classify again even if the file already stores a classification
        #[arg(long)]
        force: bool,
        /// Write the result here instead of updating the file
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Ollama model to classify with
        #[arg(long, env = "RETARJET_MODEL", default_value = classify::DEFAULT_MODEL)]
        model: String,
    },
    /// Show the skeleton, classification and clips of a GLB
    Info { file: PathBuf },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Combine {
            source,
            destination,
            overwrite,
            output,
        } => commands::combine(&source, &destination, overwrite, output.as_deref()),
        Command::Transfer {
            source,
            destination,
            pose,
            facing,
            overwrite,
            output,
            model,
        } => {
            let options = commands::TransferOptions {
                poses: &pose,
                facings: &facing,
                overwrite,
                output: output.as_deref(),
                model: &model,
            };
            commands::transfer(&source, &destination, &options)
        }
        Command::Pose {
            file,
            name,
            from,
            overwrite,
            output,
        } => commands::pose(&file, &name, from, overwrite, output.as_deref()),
        Command::Classify {
            file,
            force,
            output,
            model,
        } => commands::classify(&file, force, output.as_deref(), &model),
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
