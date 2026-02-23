use clap::Parser;
use relais_cli::{AuthAction, Cli, Commands, VaultAction};

#[test]
fn cli_parses_sites_command() {
    let cli = Cli::parse_from(["relais", "sites"]);
    assert!(matches!(cli.command, Commands::Sites));
}

#[test]
fn cli_parses_apis_command() {
    let cli = Cli::parse_from(["relais", "apis", "github"]);
    match cli.command {
        Commands::Apis { site } => assert_eq!(site, "github"),
        _ => panic!("expected Apis command"),
    }
}

#[test]
fn cli_parses_spec_command() {
    let cli = Cli::parse_from(["relais", "spec", "github.repos.list"]);
    match cli.command {
        Commands::Spec { path } => assert_eq!(path, "github.repos.list"),
        _ => panic!("expected Spec command"),
    }
}

#[test]
fn cli_parses_exec_command_without_data() {
    let cli = Cli::parse_from(["relais", "exec", "github.repos.list"]);
    match cli.command {
        Commands::Exec { path, data } => {
            assert_eq!(path, "github.repos.list");
            assert!(data.is_none());
        }
        _ => panic!("expected Exec command"),
    }
}

#[test]
fn cli_parses_exec_command_with_data() {
    let cli = Cli::parse_from([
        "relais",
        "exec",
        "github.repos.list",
        "--data",
        r#"{"owner":"rust-lang"}"#,
    ]);
    match cli.command {
        Commands::Exec { path, data } => {
            assert_eq!(path, "github.repos.list");
            assert_eq!(data.as_deref(), Some(r#"{"owner":"rust-lang"}"#));
        }
        _ => panic!("expected Exec command"),
    }
}

#[test]
fn cli_parses_serve_with_defaults() {
    let cli = Cli::parse_from(["relais", "serve"]);
    match cli.command {
        Commands::Serve { port, jwt_secret } => {
            assert_eq!(port, 3000);
            assert_eq!(jwt_secret, "dev-secret");
        }
        _ => panic!("expected Serve command"),
    }
}

#[test]
fn cli_parses_serve_with_port() {
    let cli = Cli::parse_from(["relais", "serve", "--port", "8080"]);
    match cli.command {
        Commands::Serve { port, .. } => {
            assert_eq!(port, 8080);
        }
        _ => panic!("expected Serve command"),
    }
}

#[test]
fn cli_parses_vault_store() {
    let cli = Cli::parse_from(["relais", "vault", "store", "github", "ghp_abc123"]);
    match cli.command {
        Commands::Vault { action } => match action {
            VaultAction::Store { site, token } => {
                assert_eq!(site, "github");
                assert_eq!(token, "ghp_abc123");
            }
            _ => panic!("expected Store action"),
        },
        _ => panic!("expected Vault command"),
    }
}

#[test]
fn cli_parses_vault_list() {
    let cli = Cli::parse_from(["relais", "vault", "list"]);
    match cli.command {
        Commands::Vault { action } => {
            assert!(matches!(action, VaultAction::List));
        }
        _ => panic!("expected Vault command"),
    }
}

#[test]
fn cli_parses_vault_delete() {
    let cli = Cli::parse_from(["relais", "vault", "delete", "github"]);
    match cli.command {
        Commands::Vault { action } => match action {
            VaultAction::Delete { site } => {
                assert_eq!(site, "github");
            }
            _ => panic!("expected Delete action"),
        },
        _ => panic!("expected Vault command"),
    }
}

#[test]
fn cli_parses_auth_login() {
    let cli = Cli::parse_from(["relais", "auth", "login", "github"]);
    match cli.command {
        Commands::Auth { action } => match action {
            AuthAction::Login { provider } => {
                assert_eq!(provider, "github");
            }
            _ => panic!("expected Login action"),
        },
        _ => panic!("expected Auth command"),
    }
}

#[test]
fn cli_parses_auth_custom() {
    let cli = Cli::parse_from([
        "relais",
        "auth",
        "custom",
        "--auth-url",
        "https://example.com/auth",
        "--token-url",
        "https://example.com/token",
        "--client-id",
        "myid",
        "--client-secret",
        "mysecret",
        "--site",
        "example",
    ]);
    match cli.command {
        Commands::Auth { action } => match action {
            AuthAction::Custom {
                auth_url,
                token_url,
                client_id,
                client_secret,
                site,
                scopes,
            } => {
                assert_eq!(auth_url, "https://example.com/auth");
                assert_eq!(token_url, "https://example.com/token");
                assert_eq!(client_id, "myid");
                assert_eq!(client_secret, "mysecret");
                assert_eq!(site, "example");
                assert_eq!(scopes, "");
            }
            _ => panic!("expected Custom action"),
        },
        _ => panic!("expected Auth command"),
    }
}

#[test]
fn cli_parses_auth_custom_with_scopes() {
    let cli = Cli::parse_from([
        "relais",
        "auth",
        "custom",
        "--auth-url",
        "https://example.com/auth",
        "--token-url",
        "https://example.com/token",
        "--client-id",
        "myid",
        "--client-secret",
        "mysecret",
        "--site",
        "example",
        "--scopes",
        "read,write,admin",
    ]);
    match cli.command {
        Commands::Auth { action } => match action {
            AuthAction::Custom { scopes, .. } => {
                assert_eq!(scopes, "read,write,admin");
            }
            _ => panic!("expected Custom action"),
        },
        _ => panic!("expected Auth command"),
    }
}

#[test]
fn cli_parses_auth_import_cookies() {
    let cli = Cli::parse_from([
        "relais",
        "auth",
        "import-cookies",
        "example",
        "--domain",
        "example.com",
        "--cookies",
        "session=abc; token=xyz",
    ]);
    match cli.command {
        Commands::Auth { action } => match action {
            AuthAction::ImportCookies {
                site,
                domain,
                cookies,
            } => {
                assert_eq!(site, "example");
                assert_eq!(domain, "example.com");
                assert_eq!(cookies, "session=abc; token=xyz");
            }
            _ => panic!("expected ImportCookies action"),
        },
        _ => panic!("expected Auth command"),
    }
}
