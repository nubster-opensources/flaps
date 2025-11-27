//! # Flaps CLI
//!
//! Command-line interface for Nubster Flaps.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "flaps")]
#[command(author, version, about = "Nubster Flaps CLI - Feature Flags Management", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage flags
    Flag {
        #[command(subcommand)]
        action: FlagCommands,
    },
    /// Manage projects
    Project {
        #[command(subcommand)]
        action: ProjectCommands,
    },
    /// Manage environments
    Env {
        #[command(subcommand)]
        action: EnvCommands,
    },
    /// Evaluate a flag
    Eval {
        /// Flag key
        #[arg(short, long)]
        flag: String,
        /// Environment
        #[arg(short, long, default_value = "dev")]
        env: String,
        /// User ID
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Kill switch - emergency disable a flag
    Kill {
        /// Flag key
        flag: String,
        /// Environment
        #[arg(short, long, default_value = "prod")]
        env: String,
        /// Reason for kill switch
        #[arg(short, long)]
        reason: String,
    },
    /// Export flags configuration
    Export {
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Output format (json, yaml)
        #[arg(short, long, default_value = "json")]
        format: String,
    },
    /// Import flags configuration
    Import {
        /// Input file
        file: String,
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Import mode (merge, replace, dry-run)
        #[arg(short, long, default_value = "dry-run")]
        mode: String,
    },
    /// Compare environments
    Diff {
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Source environment
        #[arg(long)]
        from: String,
        /// Target environment
        #[arg(long)]
        to: String,
    },
    /// Sync environments
    Sync {
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Source environment
        #[arg(long)]
        from: String,
        /// Target environment
        #[arg(long)]
        to: String,
    },
}

#[derive(Subcommand)]
enum FlagCommands {
    /// List flags
    List {
        /// Project key
        #[arg(short, long)]
        project: String,
    },
    /// Get flag details
    Get {
        /// Flag key
        key: String,
        /// Project key
        #[arg(short, long)]
        project: String,
    },
    /// Create a new flag
    Create {
        /// Flag key
        key: String,
        /// Flag name
        #[arg(short, long)]
        name: String,
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Flag type (boolean, string)
        #[arg(short, long, default_value = "boolean")]
        r#type: String,
    },
    /// Toggle a flag
    Toggle {
        /// Flag key
        key: String,
        /// Project key
        #[arg(short, long)]
        project: String,
        /// Environment
        #[arg(short, long)]
        env: String,
        /// Enable or disable
        #[arg(short, long)]
        enabled: bool,
    },
    /// Delete a flag
    Delete {
        /// Flag key
        key: String,
        /// Project key
        #[arg(short, long)]
        project: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// List projects
    List,
    /// Get project details
    Get {
        /// Project key
        key: String,
    },
    /// Create a new project
    Create {
        /// Project key
        key: String,
        /// Project name
        #[arg(short, long)]
        name: String,
    },
    /// Delete a project
    Delete {
        /// Project key
        key: String,
    },
}

#[derive(Subcommand)]
enum EnvCommands {
    /// List environments
    List {
        /// Project key
        #[arg(short, long)]
        project: String,
    },
    /// Create a new environment
    Create {
        /// Environment key
        key: String,
        /// Environment name
        #[arg(short, long)]
        name: String,
        /// Project key
        #[arg(short, long)]
        project: String,
    },
    /// Delete an environment
    Delete {
        /// Environment key
        key: String,
        /// Project key
        #[arg(short, long)]
        project: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Flag { action } => match action {
            FlagCommands::List { project } => {
                println!("Listing flags for project: {}", project);
            },
            FlagCommands::Get { key, project } => {
                println!("Getting flag {} in project {}", key, project);
            },
            FlagCommands::Create {
                key,
                name,
                project,
                r#type,
            } => {
                println!(
                    "Creating flag {} ({}) in project {} with type {}",
                    key, name, project, r#type
                );
            },
            FlagCommands::Toggle {
                key,
                project,
                env,
                enabled,
            } => {
                println!(
                    "Toggling flag {} in project {} env {} to {}",
                    key, project, env, enabled
                );
            },
            FlagCommands::Delete { key, project } => {
                println!("Deleting flag {} in project {}", key, project);
            },
        },
        Commands::Project { action } => match action {
            ProjectCommands::List => {
                println!("Listing projects");
            },
            ProjectCommands::Get { key } => {
                println!("Getting project {}", key);
            },
            ProjectCommands::Create { key, name } => {
                println!("Creating project {} ({})", key, name);
            },
            ProjectCommands::Delete { key } => {
                println!("Deleting project {}", key);
            },
        },
        Commands::Env { action } => match action {
            EnvCommands::List { project } => {
                println!("Listing environments for project {}", project);
            },
            EnvCommands::Create { key, name, project } => {
                println!(
                    "Creating environment {} ({}) in project {}",
                    key, name, project
                );
            },
            EnvCommands::Delete { key, project } => {
                println!("Deleting environment {} in project {}", key, project);
            },
        },
        Commands::Eval { flag, env, user } => {
            println!(
                "Evaluating flag {} in env {} for user {:?}",
                flag, env, user
            );
        },
        Commands::Kill { flag, env, reason } => {
            println!("🛑 KILL SWITCH: {} in {} - Reason: {}", flag, env, reason);
        },
        Commands::Export { project, format } => {
            println!("Exporting project {} as {}", project, format);
        },
        Commands::Import {
            file,
            project,
            mode,
        } => {
            println!(
                "Importing {} to project {} with mode {}",
                file, project, mode
            );
        },
        Commands::Diff { project, from, to } => {
            println!("Comparing {} vs {} in project {}", from, to, project);
        },
        Commands::Sync { project, from, to } => {
            println!("Syncing {} to {} in project {}", from, to, project);
        },
    }
}
