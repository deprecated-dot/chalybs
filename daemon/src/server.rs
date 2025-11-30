use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Error, Result};
use serde_json;

use crate::ipc::{IpcEvent, IpcEventKind, IpcMessage, IpcVmState};
use crate::state::{map_vm_state_to_ipc, DaemonState, DaemonVm};

// ---- Chalybs core imports (read-only introspection / VM status) -----------

use chalybs_core::config::IsolationMode;

// ---------------------------------------------------------------------------

const PRIMARY_SOCKET: &str = "/run/chalybsd.sock";
const FALLBACK_SOCKET: &str = "/tmp/chalybsd.sock";

fn choose_socket_path() -> PathBuf {
    let run = Path::new("/run");
    if run.is_dir() {
        PathBuf::from(PRIMARY_SOCKET)
    } else {
        PathBuf::from(FALLBACK_SOCKET)
    }
}

/// A connected TUI client.
///
/// Each client has its own stream and receive buffer for decoding
/// newline-delimited JSON messages coming from the TUI (shell commands).
struct Client {
    stream: UnixStream,
    rx_buf: String,
}

impl Client {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Client {
            stream,
            rx_buf: String::new(),
        })
    }
}

/// Outcome of draining commands from a single client.
enum ClientDrainOutcome {
    /// Client is still connected and usable.
    Alive,
    /// Client closed the connection cleanly (EOF or expected disconnect).
    DisconnectedGraceful,
    /// Client encountered an error and should be dropped.
    DisconnectedError(Error),
}

/// Run the chalybsd IPC server in multi-client, push-snapshot mode.
///
/// Behavior:
/// - Binds a Unix-domain socket (preferring /run, falling back to /tmp).
/// - Accepts multiple TUI clients (non-blocking).
/// - Builds snapshots from real Chalybs config + PCI inventory, and
///   drives real core VmStateMachines for each started VM.
/// - On each tick:
///   - Reads any pending shell commands from all clients.
///   - Applies shell commands to DaemonState as VM intent (desired
///     VM states).
///   - Advances VmStateMachines one step towards their desired state.
///   - Builds a fresh snapshot.
///   - Sends the same Snapshot to all connected clients.
/// - On any I/O error or EOF, drops the offending client and continues.
///
/// This keeps the control flow single-threaded and deterministic while
/// still allowing multiple simultaneous TUIs.
pub fn run_server() -> Result<()> {
    let path = choose_socket_path();

    // Remove any stale socket.
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove stale socket at {}", path.display()))?;
    }

    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;

    listener
        .set_nonblocking(true)
        .context("failed to set listener nonblocking")?;

    tracing::info!("chalybsd: listening on {}", path.display());
    tracing::info!("chalybsd: multi-client mode enabled (single-threaded)");

    // Phase 11: build real daemon state with config + PCI introspection.
    let mut daemon_state = DaemonState::new();

    let mut clients: Vec<Client> = Vec::new();
    let mut tick: u64 = 0;

    // Shell-related buffers:
    // - pending_shell_events: echoes + parser errors, etc.
    // - pending_shell_commands: raw commands for semantic handling.
    let mut pending_shell_events: Vec<IpcEvent> = Vec::new();
    let mut pending_shell_commands: Vec<String> = Vec::new();

    loop {
        // 1. Accept new clients (non-blocking).
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    let _ = addr; // Unused for Unix-domain sockets.
                    match Client::new(stream) {
                        Ok(client) => {
                            tracing::info!(
                                "chalybsd: new TUI client connected (total={})",
                                clients.len() + 1
                            );
                            clients.push(client);
                        }
                        Err(err) => {
                            tracing::error!("chalybsd: failed to initialize client: {err}");
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    tracing::error!("chalybsd: accept error: {e}");
                    break;
                }
            }
        }

        // 2. Read commands from all clients.
        //
        // Any ShellCommand results in:
        //   - Echo events added to `pending_shell_events`.
        //   - Raw command strings added to `pending_shell_commands`.
        // These will be broadcast as part of the next snapshot.
        let mut i = 0;
        while i < clients.len() {
            let outcome = drain_client_commands(
                &mut clients[i],
                &mut pending_shell_events,
                &mut pending_shell_commands,
            );
            match outcome {
                ClientDrainOutcome::Alive => {
                    i += 1;
                }
                ClientDrainOutcome::DisconnectedGraceful => {
                    tracing::info!("chalybsd: client disconnected gracefully; removing");
                    clients.remove(i);
                }
                ClientDrainOutcome::DisconnectedError(err) => {
                    tracing::warn!("chalybsd: dropping client due to error: {err}");
                    clients.remove(i);
                }
            }
        }

        // 3. Handle parsed shell commands semantically and update
        //    daemon_state's VM desired states before building the
        //    snapshot and advancing state machines.
        let mut shell_semantic_events: Vec<IpcEvent> = Vec::new();
        if !pending_shell_commands.is_empty() {
            for cmd in pending_shell_commands.drain(..) {
                let mut cmd_events = handle_shell_command(&mut daemon_state, cmd.as_str());
                shell_semantic_events.append(&mut cmd_events);
            }
        }

        // 4. Advance VMs one tick and build a real snapshot.
        tick = tick.wrapping_add(1);

        let (mut vms, mut events_global, events_vm) = daemon_state.build_snapshot(tick);

        // First, inject any shell echo / decode events so they appear
        // before semantic command handling.
        if !pending_shell_events.is_empty() {
            events_global.append(&mut pending_shell_events);
        }

        // Next, inject semantic shell events (vm list/status/start/etc).
        if !shell_semantic_events.is_empty() {
            events_global.append(&mut shell_semantic_events);
        }

        // 5. Serialize the snapshot once.
        let msg = IpcMessage::Snapshot {
            vms: std::mem::take(&mut vms),
            events_global,
            events_vm,
        };

        let json = match serde_json::to_string(&msg) {
            Ok(j) => j,
            Err(err) => {
                tracing::error!("chalybsd: failed to serialize Snapshot: {err}");
                // If we can't serialize, skip this tick rather than
                // poisoning all clients.
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        };

        let payload = format!("{json}\n");

        // 6. Broadcast to all clients; drop any that fail on write.
        let mut j = 0;
        while j < clients.len() {
            let write_result = clients[j].stream.write_all(payload.as_bytes());
            match write_result {
                Ok(()) => {
                    // Best-effort flush; ignore errors here.
                    let _ = clients[j].stream.flush();
                    j += 1;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::BrokenPipe
                        || e.kind() == io::ErrorKind::ConnectionReset
                        || e.kind() == io::ErrorKind::ConnectionAborted =>
                {
                    tracing::info!("chalybsd: client disconnected during write: {e}");
                    clients.remove(j);
                }
                Err(e) => {
                    tracing::warn!("chalybsd: write error to client: {e}");
                    clients.remove(j);
                }
            }
        }

        // 7. Push-model heartbeat: fixed interval.
        thread::sleep(Duration::from_millis(250));
    }
}

/// Drain all available input from a single client, decoding any
/// newline-delimited JSON messages and updating the global
/// `pending_shell_events` and `pending_shell_commands` buffers.
///
/// Returns:
/// - Alive on success
/// - DisconnectedGraceful if the client closed the connection (EOF or
///   expected disconnection error kinds)
/// - DisconnectedError(_) if the client should be dropped due to a hard error
fn drain_client_commands(
    client: &mut Client,
    pending_shell_events: &mut Vec<IpcEvent>,
    pending_shell_commands: &mut Vec<String>,
) -> ClientDrainOutcome {
    let mut buf = [0_u8; 4096];

    loop {
        match client.stream.read(&mut buf) {
            Ok(0) => {
                // EOF: client closed connection cleanly.
                tracing::info!("chalybsd: client closed connection (EOF)");
                return ClientDrainOutcome::DisconnectedGraceful;
            }
            Ok(n) => {
                let chunk = match std::str::from_utf8(&buf[..n]) {
                    Ok(c) => c,
                    Err(e) => {
                        let err = Error::new(e).context("invalid UTF-8 from client");
                        return ClientDrainOutcome::DisconnectedError(err);
                    }
                };

                client.rx_buf.push_str(chunk);

                while let Some(idx) = client.rx_buf.find('\n') {
                    // Take an owned copy of the line slice to avoid
                    // borrow conflicts when draining from `rx_buf`.
                    let raw = client.rx_buf[..idx].to_string();
                    client.rx_buf.drain(..=idx);

                    let line = raw.trim();
                    if line.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<IpcMessage>(line) {
                        Ok(IpcMessage::ShellCommand { command }) => {
                            tracing::info!("chalybsd: shell command from TUI: {command}");

                            // Shell echo event so the TUI always shows the
                            // command the user typed.
                            pending_shell_events.push(IpcEvent {
                                kind: IpcEventKind::Shell,
                                message: format!("chalybs> {command}"),
                            });

                            // Defer semantic handling to the main loop, where
                            // we have access to DaemonState (config/inventory).
                            pending_shell_commands.push(command);
                        }
                        Ok(IpcMessage::Snapshot { .. }) => {
                            // We do not expect Snapshot from TUI; ignore.
                            tracing::warn!("chalybsd: unexpected Snapshot message from client");
                        }
                        Err(err) => {
                            tracing::error!(
                                "chalybsd: failed to decode IPC message from client: {err}"
                            );
                            pending_shell_events.push(IpcEvent {
                                kind: IpcEventKind::Error,
                                message: format!("daemon: invalid IPC message: {err}"),
                            });
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // No more data for now.
                break;
            }
            Err(e)
                if e.kind() == io::ErrorKind::ConnectionReset
                    || e.kind() == io::ErrorKind::ConnectionAborted =>
            {
                // These are expected when the client exits abruptly but
                // without protocol or data corruption. Treat as graceful.
                tracing::info!("chalybsd: client disconnected (read error: {e})");
                return ClientDrainOutcome::DisconnectedGraceful;
            }
            Err(e) => {
                let err = Error::new(e).context("read error from client");
                return ClientDrainOutcome::DisconnectedError(err);
            }
        }
    }

    ClientDrainOutcome::Alive
}

// ---------------------------------------------------------------------------
// Helper: shell command semantic handling (Phase 11 — real VMs)
// ---------------------------------------------------------------------------

fn handle_shell_command(state: &mut DaemonState, command: &str) -> Vec<IpcEvent> {
    let mut events = Vec::new();
    let line = command.trim();

    if line.is_empty() {
        return events;
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return events;
    }

    match parts[0] {
        "help" | "h" | "?" => {
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: "available commands: help | vm list | vm status <name> | vm start <name> | vm stop <name> | vm restart <name>".to_string(),
            });
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: "note: VM lifecycle is now driven by real state machines; starts/stops will affect actual VMs"
                    .to_string(),
            });
        }

        "vm" => handle_vm_command(state, &parts[1..], &mut events),

        other => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!("daemon: unknown command `{other}` (try `help`)"),
            });
        }
    }

    events
}

fn handle_vm_command(state: &mut DaemonState, args: &[&str], events: &mut Vec<IpcEvent>) {
    if args.is_empty() {
        events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: "usage: vm <list|status|start|stop|restart> [name]".to_string(),
        });
        return;
    }

    let Some(ref cfg) = state.root_cfg else {
        events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: "daemon: no config loaded; vm commands are unavailable".to_string(),
        });
        return;
    };

    match args[0] {
        "list" => {
            if cfg.vm.is_empty() {
                events.push(IpcEvent {
                    kind: IpcEventKind::Info,
                    message: "daemon: no VMs defined in configuration".to_string(),
                });
                return;
            }

            let mut names: Vec<&String> = cfg.vm.keys().collect();
            names.sort();

            let joined = names
                .into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!("daemon: configured VMs: {joined}"),
            });
        }

        "status" => {
            if args.len() < 2 {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: "usage: vm status <name>".to_string(),
                });
                return;
            }

            let name = args[1];
            match cfg.vm.get(name) {
                Some(vm) => {
                    let isolation_mode = match vm.isolation.mode {
                        IsolationMode::Disabled => "disabled",
                        IsolationMode::Audit => "audit",
                        IsolationMode::Enforce => "enforce",
                    };

                    let gpu_count = vm.devices.gpu.as_ref().map(|v| v.len()).unwrap_or(0);
                    let nvme_count = vm.devices.nvme.as_ref().map(|v| v.len()).unwrap_or(0);
                    let nic_count = vm.devices.nic.as_ref().map(|v| v.len()).unwrap_or(0);
                    let usb_count = vm.devices.usb.as_ref().map(|v| v.len()).unwrap_or(0);

                    let (view_state, desired_state) = if let Some(dvm) = state.vms.get(name) {
                        (map_vm_state_to_ipc(dvm.sm.state), dvm.desired)
                    } else {
                        (IpcVmState::Stopped, IpcVmState::Stopped)
                    };

                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {name}: state={view_state:?}, desired={desired_state:?}, isolation={isolation_mode}, hugepages={}",
                            vm.qemu.hugepages
                        ),
                    });

                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {name}: devices gpu={gpu_count}, nvme={nvme_count}, nic={nic_count}, usb={usb_count}"
                        ),
                    });
                }
                None => {
                    events.push(IpcEvent {
                        kind: IpcEventKind::Error,
                        message: format!("vm {name}: not found in configuration"),
                    });
                }
            }
        }

        "start" | "restart" => {
            if args.len() < 2 {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!("usage: vm {} <name>", args[0]),
                });
                return;
            }

            let action = args[0];
            let name = args[1];

            let vm_cfg = match cfg.vm.get(name) {
                Some(vm) => vm,
                None => {
                    events.push(IpcEvent {
                        kind: IpcEventKind::Error,
                        message: format!("vm {name}: not found in configuration"),
                    });
                    return;
                }
            };

            // If we already have a DaemonVm and it is Idle from a
            // previous run, reinitialize it to get a fresh VmRuntime.
            let need_new = match state.vms.get(name) {
                None => true,
                Some(dvm) => matches!(dvm.sm.state, chalybs_core::state::VmState::Idle),
            };

            if need_new {
                match DaemonVm::new(name, vm_cfg) {
                    Ok(dvm) => {
                        state.vms.insert(name.to_string(), dvm);
                    }
                    Err(msg) => {
                        events.push(IpcEvent {
                            kind: IpcEventKind::Error,
                            message: msg,
                        });
                        return;
                    }
                }
            }

            let dvm = state
                .vms
                .get_mut(name)
                .expect("DaemonVm must exist after construction");

            dvm.desired = IpcVmState::Starting;

            events.push(IpcEvent {
                kind: IpcEventKind::System,
                message: format!("vm {name}: `{action}` requested; desired state set to Starting"),
            });
        }

        "stop" => {
            if args.len() < 2 {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: "usage: vm stop <name>".to_string(),
                });
                return;
            }

            let name = args[1];

            if !cfg.vm.contains_key(name) {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!("vm {name}: not found in configuration"),
                });
                return;
            }

            match state.vms.get_mut(name) {
                Some(dvm) => {
                    dvm.desired = IpcVmState::ShuttingDown;
                    events.push(IpcEvent {
                        kind: IpcEventKind::System,
                        message: format!(
                            "vm {name}: `stop` requested; desired state set to ShuttingDown"
                        ),
                    });
                }
                None => {
                    // VM has never been started in this daemon run; treat
                    // as a no-op but make it visible.
                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {name}: stop requested but VM has no active state machine; ignoring"
                        ),
                    });
                }
            }
        }

        other => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!(
                    "daemon: unknown vm subcommand `{other}` (expected list|status|start|stop|restart)"
                ),
            });
        }
    }
}
