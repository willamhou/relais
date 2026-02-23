pub mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "relais", about = "The agent internet gateway", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// List all available sites
    Sites,
    /// List APIs for a site
    Apis {
        /// Site ID (e.g., "github")
        site: String,
    },
    /// Show action specification
    Spec {
        /// Spec path (e.g., "github.repos.list")
        path: String,
    },
    /// Execute an action
    Exec {
        /// Action path (e.g., "github.repos.list")
        path: String,
        /// JSON data for params
        #[arg(long)]
        data: Option<String>,
    },
    /// Start the HTTP API server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "3000")]
        port: u16,
        /// JWT secret
        #[arg(long, env = "RELAIS_JWT_SECRET", default_value = "dev-secret")]
        jwt_secret: String,
    },
    /// Manage credential vault
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
    /// Authenticate with a site via OAuth or cookie import
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
}

#[derive(Subcommand)]
pub enum VaultAction {
    /// Store a credential
    Store {
        /// Site ID (e.g., "github")
        site: String,
        /// The credential token to store
        token: String,
    },
    /// List stored credentials
    List,
    /// Delete a credential
    Delete {
        /// Site ID to delete
        site: String,
    },
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// OAuth login for a known provider (e.g., github)
    Login {
        /// Provider name (e.g., "github")
        provider: String,
    },
    /// Custom OAuth login with explicit parameters
    Custom {
        /// OAuth authorization URL
        #[arg(long)]
        auth_url: String,
        /// OAuth token URL
        #[arg(long)]
        token_url: String,
        /// Client ID
        #[arg(long)]
        client_id: String,
        /// Client secret
        #[arg(long)]
        client_secret: String,
        /// Site ID to store credential under
        #[arg(long)]
        site: String,
        /// Scopes (comma-separated)
        #[arg(long, default_value = "")]
        scopes: String,
    },
    /// Import cookies from browser (manual)
    ImportCookies {
        /// Site ID
        site: String,
        /// Domain the cookies belong to
        #[arg(long)]
        domain: String,
        /// Cookie string (name=value pairs, semicolon-separated)
        #[arg(long)]
        cookies: String,
    },
}
