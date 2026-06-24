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
        /// Host/interface to bind. Defaults to loopback; set 0.0.0.0 to expose.
        #[arg(long, env = "RELAIS_HOST", default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on
        #[arg(long, default_value = "3000")]
        port: u16,
        /// JWT secret (required; no insecure default). Use ≥32 chars.
        #[arg(long, env = "RELAIS_JWT_SECRET")]
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
    /// Cryptographic call auditing (signet): keys, verification, log tail
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
}

#[derive(Subcommand)]
pub enum AuditAction {
    /// Generate (or load) the gateway signing key and print its public key
    Init {
        /// Owner recorded in receipts (defaults to RELAIS_AUDIT_OWNER, then "relais")
        #[arg(long)]
        owner: Option<String>,
    },
    /// Print the gateway public key (`ed25519:<base64>`)
    Pubkey,
    /// Verify the audit chain against the trusted-key anchor (fail-closed)
    Verify {
        /// Expected chain head (record_hash) retained out-of-band; detects tail truncation
        #[arg(long)]
        head: Option<String>,
    },
    /// List recent audit records
    Tail {
        /// Only records for this site (matches the `site.` tool prefix)
        #[arg(long)]
        site: Option<String>,
        /// Only records at or after this RFC 3339 timestamp
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of records to show
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
pub enum VaultAction {
    /// Store a credential
    Store {
        /// Site ID (e.g., "github")
        site: String,
        /// The token (DEPRECATED: exposed in shell history; prefer --token-file or stdin)
        token: Option<String>,
        /// Read the token from a file
        #[arg(long)]
        token_file: Option<String>,
    },
    /// List stored credentials
    List,
    /// Delete a credential
    Delete {
        /// Site ID to delete
        site: String,
    },
    /// Re-encrypt all credentials into the current (v1) vault format
    Migrate,
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
        /// Client secret (DEPRECATED on CLI; prefer --client-secret-file or stdin)
        #[arg(long)]
        client_secret: Option<String>,
        /// Read the client secret from a file
        #[arg(long)]
        client_secret_file: Option<String>,
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
        /// Cookie string (DEPRECATED on CLI; prefer --cookies-file or stdin)
        #[arg(long)]
        cookies: Option<String>,
        /// Read the cookie string from a file
        #[arg(long)]
        cookies_file: Option<String>,
    },
}
