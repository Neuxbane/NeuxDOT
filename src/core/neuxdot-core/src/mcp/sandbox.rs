//! Default-on isolation for spawned plugin processes.
//!
//! ## Why
//!
//! A `stdio` or `binary` plugin is an arbitrary executable the account
//! installed. Before this module it was spawned with the host's full
//! filesystem, the host's network and the host's credentials, which made
//! "install a plugin" equivalent to "run this code as me".
//!
//! Every plugin process now starts inside a bubblewrap sandbox:
//!
//! | Resource | Sandbox default |
//! | --- | --- |
//! | Host filesystem | Not visible. `/usr`, `/bin`, `/lib` are read-only bind mounts (the runtime it needs to execute), `/home` does not exist |
//! | Working directory | A per-plugin `/workspace` — the only writable path |
//! | Network | No network at all unless the manifest allow-lists destinations |
//! | `/tmp` | A private tmpfs |
//! | PIDs and session | Own namespace, dies with the parent |
//!
//! A plugin that genuinely needs the host (a terminal tool, a package manager)
//! declares `permissions.elevated = true`, which is surfaced in the consent UI
//! and skips [`spawn_command`] wrapping entirely. The operator can also grant
//! or revoke that one bit explicitly in the plugin editor.
//!
//! ## Network
//!
//! The sandbox always runs `--unshare-all`, so the network namespace starts
//! empty. Grants are applied on top:
//!
//! * **denied** (default) — nothing further. Connections fail with
//!   `Network is unreachable`.
//! * **allow-list** — the host starts a CONNECT proxy on a unix socket, binds
//!   that socket into the sandbox, and relays `127.0.0.1:PORT` to it via
//!   `socat`. The child gets `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` plus the
//!   proxy is the *only* reachable destination. Enforcement is in
//!   [`crate::mcp::proxy::AllowListProxy`], which refuses any host not on the
//!   list.
//! * **`["*"]`** — `--share-net`, i.e. the host's network stack. The UI shows a
//!   soft warning; this is the plugin escaping the isolation boundary.
//!
//! The allow-list proxy covers HTTP clients that honour proxy environment
//! variables (reqwest, curl, python-requests, node-fetch, …). A plugin that
//! opens a raw socket is still confined, because the sandbox has no route
//! except loopback. It just cannot reach anything that way.

use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::db::schema::{McpServer, NetworkPermissions};

/// Where plugin workspaces live, relative to the server's working directory.
pub const SANDBOX_DATA_DIR: &str = "data/sandbox";

/// Mount point of the plugin's private, writable directory.
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// Where an installed plugin binary is bound inside the sandbox.
///
/// A fixed path, not the host path: the sandbox has no `/home` or `/data`, and
/// binding the host path would leak the host's directory layout.
pub const PROGRAM_MOUNT: &str = "/run/neuxdot-plugin";

/// In-sandbox loopback port the relay listens on when a network allow-list is
/// configured. Chosen high enough to dodge the common dev ports.
pub const RELAY_PORT: u16 = 41_873;

/// How a plugin process should be launched.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// Run inside the bubblewrap sandbox described by `spec`.
    Sandboxed(SandboxSpec),
    /// Run directly on the host. Only for plugins that explicitly asked for it.
    Elevated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SandboxSpec {
    /// Host directory bind-mounted at [`WORKSPACE_MOUNT`].
    pub workspace: PathBuf,
    /// Host path of the executable to run, bound read-only at
    /// [`PROGRAM_MOUNT`].
    ///
    /// Needed because a plugin installed into `data/binaries/` (or run from a
    /// checkout under `/home`) lives outside the `/usr` bind, so the sandbox
    /// cannot see it otherwise. `None` means the program is resolved on the
    /// sandbox `PATH`.
    pub program: Option<PathBuf>,
    /// What the sandboxed process may reach.
    pub network: NetworkPermissions,
    /// Unix socket of the allow-list proxy, bound into the sandbox.
    ///
    /// Only set when `network` is a non-empty, non-wildcard allow-list.
    pub proxy_socket: Option<PathBuf>,
}

impl SandboxSpec {
    /// The workspace directory for one plugin, created if missing.
    pub fn workspace_for(server_id: &str) -> PathBuf {
        let safe: String = server_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let dir = PathBuf::from(SANDBOX_DATA_DIR).join(safe).join("workspace");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Decide how `server` must be launched.
    ///
    /// `elevated` wins over everything: the plugin declared host access, or the
    /// operator granted it explicitly, so the consent UI showed the amber
    /// warning.
    pub fn launch_for(server: &McpServer) -> Launch {
        let perms = server.get_permissions();
        if perms.elevated {
            return Launch::Elevated;
        }

        let network = perms.network.clone();
        let proxy_socket = if network.is_denied() || network.is_unrestricted() {
            None
        } else {
            Some(Self::socket_path(&server.id))
        };

        Launch::Sandboxed(SandboxSpec {
            workspace: Self::workspace_for(&server.id),
            program: Self::program_path(&server.url_or_command),
            network,
            proxy_socket,
        })
    }

    /// Host path of the executable in `command_line`, when it is one.
    ///
    /// `bwrap` needs the real file to bind. A bare name (`npx`, `node`) resolves
    /// through the sandbox `PATH` instead, so `None` is returned for it.
    pub fn program_path(command_line: &str) -> Option<PathBuf> {
        let program = command_line.split_whitespace().next()?;
        let path = Path::new(program);
        if !(program.contains('/') || program.starts_with('.')) {
            return None;
        }
        // Canonicalise so a relative `./data/binaries/x` becomes bindable even
        // though the sandbox's cwd is /workspace.
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        Some(absolute.canonicalize().unwrap_or(absolute))
    }

    /// Stable unix-socket path for one plugin's allow-list proxy.
    ///
    /// `std::env::temp_dir()` rather than the workspace, because the socket is
    /// created before the sandbox exists and lives for the process lifetime.
    pub fn socket_path(server_id: &str) -> PathBuf {
        let safe: String = server_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        std::env::temp_dir().join(format!("neuxdot-mcp-proxy-{safe}.sock"))
    }

    /// True when this sandbox has the host network stack.
    pub fn shares_host_network(&self) -> bool {
        self.network.is_unrestricted()
    }

    /// True when the child must be given proxy configuration.
    pub fn uses_proxy(&self) -> bool {
        self.proxy_socket.is_some() && !self.network.is_denied()
    }
}

/// Build the `Command` that launches `program` with `args` under `spec`.
///
/// Returns the command plus, when a relay is needed, the `socat` argv that has
/// to run **inside** the sandbox before the plugin starts. The caller spawns
/// the relay first (see [`sandbox_argv`] usage in the stdio transport).
pub fn sandboxed_command(program: &str, args: &[String], spec: &SandboxSpec) -> Command {
    let mut cmd = Command::new("bwrap");
    for arg in sandbox_argv(spec) {
        cmd.arg(arg);
    }
    // Inside the sandbox the program only exists at PROGRAM_MOUNT.
    let in_sandbox_program = if spec.program.is_some() {
        PROGRAM_MOUNT
    } else {
        program
    };
    cmd.arg("--");
    for arg in inner_argv(in_sandbox_program, args, spec) {
        cmd.arg(arg);
    }

    // Environment that makes the sandbox usable and the proxy discoverable.
    cmd.env("HOME", WORKSPACE_MOUNT)
        .env("TMPDIR", "/tmp")
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        // Some runtimes insist on a writable XDG dir; keep them in the workspace.
        .env("XDG_CACHE_HOME", "/workspace/.cache")
        .env("XDG_CONFIG_HOME", "/workspace/.config");

    if spec.uses_proxy() {
        let url = format!("http://127.0.0.1:{RELAY_PORT}");
        cmd.env("HTTP_PROXY", &url)
            .env("HTTPS_PROXY", &url)
            .env("ALL_PROXY", &url)
            .env("http_proxy", &url)
            .env("https_proxy", &url)
            .env("all_proxy", &url)
            // reqwest and curl read NO_PROXY; an empty value keeps the proxy in
            // charge even for "localhost" style requests.
            .env("NO_PROXY", "")
            .env("no_proxy", "");
    }

    cmd
}

/// The argv of the process that bubblewrap actually executes.
///
/// Normally this is the plugin itself. When a network allow-list is in force it
/// is `sh -c "<relay> & exec plugin ...">`, because the relay has to listen on
/// **the sandbox's** loopback (the sandbox has its own network namespace, so the
/// host cannot listen there for it) and the plugin must keep the stdin/stdout
/// pipes the MCP transport speaks over.
///
/// The plugin is passed as `$0`/`$@`, never interpolated into the script, so an
/// argument containing quotes or spaces cannot break out of the shell.
pub fn inner_argv(program: &str, args: &[String], spec: &SandboxSpec) -> Vec<String> {
    if !spec.uses_proxy() {
        let mut argv = vec![program.to_string()];
        argv.extend(args.iter().cloned());
        return argv;
    }

    let relay = relay_argv();
    let script = format!(
        "socat {} {} >/dev/null 2>&1 & sleep 0.2; exec \"$0\" \"$@\"",
        shell_quote(&relay[0]),
        shell_quote(&relay[1])
    );
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script,
        // `sh -c script name arg...` puts `name` into $0.
        program.to_string(),
    ];
    argv.extend(args.iter().cloned());
    argv
}

/// Quote one word for `sh`. Only used for constants this module builds, never
/// for untrusted input — but quoting keeps the script readable and correct.
fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:,=+@".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The bubblewrap arguments that describe the sandbox, up to (not including)
/// the `--` separator.
///
/// Exposed separately so the stdio transport can prepend the relay setup to the
/// same `bwrap` invocation and so tests can assert the confinement flags
/// without spawning anything.
pub fn sandbox_argv(spec: &SandboxSpec) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        // Everything: user, pid, net, ipc, uts, cgroup, mnt namespaces.
        "--unshare-all".into(),
        // No surviving daemon if the host goes away.
        "--die-with-parent".into(),
        // Detach from the controlling terminal.
        "--new-session".into(),
    ];

    // The runtime: read-only, so a plugin can execute but not modify host
    // binaries. `/usr` carries /bin and /lib on modern distros, so the
    // traditional top-level paths become symlinks.
    for dir in ["/usr"] {
        if Path::new(dir).exists() {
            argv.push("--ro-bind".into());
            argv.push(dir.into());
            argv.push(dir.into());
        }
    }
    for (link, target) in [
        ("/bin", "usr/bin"),
        ("/sbin", "usr/sbin"),
        ("/lib", "usr/lib"),
        ("/lib64", "usr/lib64"),
    ] {
        // Only create the compat symlink when the host has the target; a distro
        // without /usr/lib64 must not get a dangling /lib64.
        if Path::new("/").join(target).exists() {
            argv.push("--symlink".into());
            argv.push(target.into());
            argv.push(link.into());
        }
    }

    // TLS roots and name resolution are read-only and inert without a network
    // grant, but needed the moment one is given.
    for path in [
        "/etc/ssl",
        "/etc/ca-certificates",
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/group",
        // The dynamic loader consults this to find the .so files a plugin
        // binary outside /usr links against.
        "/etc/ld.so.cache",
    ] {
        if Path::new(path).exists() {
            argv.push("--ro-bind".into());
            argv.push(path.into());
            argv.push(path.into());
        }
    }

    argv.push("--proc".into());
    argv.push("/proc".into());
    argv.push("--dev".into());
    argv.push("/dev".into());
    argv.push("--tmpfs".into());
    argv.push("/tmp".into());

    // The one writable place, and the working directory.
    argv.push("--bind".into());
    argv.push(spec.workspace.display().to_string());
    argv.push(WORKSPACE_MOUNT.into());
    argv.push("--chdir".into());
    argv.push(WORKSPACE_MOUNT.into());

    // The plugin executable itself. Bound read-only at a fixed path because a
    // binary plugin lives under data/binaries/ (outside /usr), and because
    // binding the real host path would advertise the host's layout.
    if let Some(program) = &spec.program {
        if program.exists() {
            argv.push("--ro-bind".into());
            argv.push(program.display().to_string());
            argv.push(PROGRAM_MOUNT.into());
        }
    }

    // The proxy socket, when an allow-list is in force.
    if let Some(socket) = &spec.proxy_socket {
        if socket.exists() {
            argv.push("--bind".into());
            argv.push(socket.display().to_string());
            argv.push(PROXY_SOCKET_MOUNT.into());
        }
    }

    if spec.shares_host_network() {
        // Escape hatch: the plugin gets the host's network stack.
        argv.push("--share-net".into());
    }

    argv
}

/// Where the allow-list proxy socket appears inside the sandbox.
pub const PROXY_SOCKET_MOUNT: &str = "/run/neuxdot-proxy.sock";

/// The `socat` argv that relays `RELAY_PORT` on loopback to the proxy socket.
///
/// Runs inside the sandbox: loopback exists there by default, but nothing else
/// is reachable, so this is the only route out.
pub fn relay_argv() -> Vec<String> {
    vec![
        format!("TCP-LISTEN:{RELAY_PORT},bind=127.0.0.1,fork,reuseaddr"),
        format!("UNIX-CONNECT:{PROXY_SOCKET_MOUNT}"),
    ]
}

/// A human-readable one-line description of what a plugin's process may do.
/// Used by the consent UI and by startup logging so the policy is visible.
pub fn describe(spec: &SandboxSpec) -> String {
    let net = if spec.network.is_denied() {
        "no network".to_string()
    } else if spec.network.is_unrestricted() {
        "UNRESTRICTED network".to_string()
    } else {
        format!("network allow-list {:?}", spec.network.allow)
    };
    format!(
        "sandboxed (workspace {}, {net}, read-only /usr)",
        spec.workspace.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(network: NetworkPermissions) -> SandboxSpec {
        SandboxSpec {
            workspace: PathBuf::from("/tmp/ws"),
            program: None,
            network,
            proxy_socket: None,
        }
    }

    #[test]
    fn argv_always_unshares_every_namespace() {
        let argv = sandbox_argv(&spec(NetworkPermissions::default()));
        let joined = argv.join(" ");
        assert!(joined.contains("--unshare-all"));
        assert!(joined.contains("--die-with-parent"));
        // No host network by default.
        assert!(!joined.contains("--share-net"));
    }

    #[test]
    fn workspace_is_the_only_writable_bind() {
        let argv = sandbox_argv(&spec(NetworkPermissions::default()));
        let joined = argv.join(" ");
        assert!(joined.contains("--bind /tmp/ws /workspace"));
        assert!(joined.contains("--chdir /workspace"));
        // A writable bind of a host path would defeat the sandbox.
        assert!(!joined.contains("--bind /home"));
        assert!(!joined.contains("--bind / "));
    }

    #[test]
    fn host_home_is_not_mounted() {
        let argv = sandbox_argv(&spec(NetworkPermissions::default()));
        assert!(!argv.iter().any(|a| a == "/home" || a == "/root"));
    }

    #[test]
    fn unrestricted_network_reintroduces_the_host_stack() {
        let unrestricted = NetworkPermissions {
            allow: vec!["*".to_string()],
        };
        assert!(sandbox_argv(&spec(unrestricted))
            .join(" ")
            .contains("--share-net"));
    }

    #[test]
    fn allow_list_gets_a_proxy_but_no_host_network() {
        let mut s = spec(NetworkPermissions {
            allow: vec!["api.example.com".into()],
        });
        s.proxy_socket = Some(PathBuf::from("/nonexistent.sock"));
        let joined = sandbox_argv(&s).join(" ");
        assert!(!joined.contains("--share-net"));
        // The socket does not exist in this test, so it is not bound; the
        // decision to use a proxy is still taken.
        assert!(s.uses_proxy());
        assert!(!s.shares_host_network());
    }

    #[test]
    fn relay_targets_loopback_only() {
        let argv = relay_argv();
        assert!(argv[0].contains("bind=127.0.0.1"));
        assert!(argv[0].contains(&RELAY_PORT.to_string()));
        assert!(argv[1].starts_with("UNIX-CONNECT:"));
    }

    #[test]
    fn executable_outside_usr_is_bound_at_a_fixed_path() {
        let mut s = spec(NetworkPermissions::default());
        // Use a path that exists so the bind is emitted.
        s.program = Some(PathBuf::from("/bin/sh"));
        let argv = sandbox_argv(&s);
        let joined = argv.join(" ");
        assert!(joined.contains("--ro-bind /bin/sh /run/neuxdot-plugin"));
        // The host path itself must not be exposed under its own name.
        assert!(!joined.contains("--ro-bind /home"));
    }

    #[test]
    fn unbound_program_runs_from_the_sandbox_path() {
        let s = spec(NetworkPermissions::default());
        let argv = sandbox_argv(&s);
        assert!(!argv.join(" ").contains("neuxdot-plugin"));
    }

    #[test]
    fn program_path_resolves_only_real_paths() {
        // A bare name is resolved by PATH inside the sandbox.
        assert_eq!(SandboxSpec::program_path("node --x"), None);
        assert_eq!(SandboxSpec::program_path("npx -y foo"), None);
        // An absolute existing path is canonicalised, so a symlinked binary is
        // bound by its real target (the sandbox has no reason to recreate the
        // host's symlink chain).
        let resolved = SandboxSpec::program_path("/bin/sh").expect("resolves");
        assert!(resolved.is_absolute());
        assert!(resolved.exists());
        assert_eq!(resolved, resolved.canonicalize().unwrap());
        // A missing path is still returned so the error is explicit at exec time.
        assert!(SandboxSpec::program_path("/nonexistent/plugin").is_some());
    }

    #[test]
    fn describe_names_the_network_policy() {
        assert!(describe(&spec(NetworkPermissions::default())).contains("no network"));
        assert!(describe(&spec(NetworkPermissions {
            allow: vec!["*".into()]
        }))
        .contains("UNRESTRICTED"));
        assert!(describe(&spec(NetworkPermissions {
            allow: vec!["a.example".into()]
        }))
        .contains("a.example"));
    }

    #[test]
    fn socket_path_is_sanitised() {
        let path = SandboxSpec::socket_path("../../etc/passwd");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains(".."));
        assert!(!name.contains('/'));
    }
}
