#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Headless updater check: prints {current_version, available, version, …}
    // as JSON and exits. Honours AGENT_MONITOR_UPDATE_ENDPOINT (test/mirror).
    if args.iter().any(|a| a == "--check-update") {
        agent_monitor::update::check_update_headless();
        return;
    }
    if args.iter().any(|a| a == "--dump-tasks") {
        agent_monitor::dump_tasks();
        return;
    }
    if args.iter().any(|a| a == "--dump-agents") {
        agent_monitor::dump_agents();
        return;
    }
    if args.iter().any(|a| a == "--jump-diagnostics") {
        agent_monitor::dump_jump_diagnostics();
        return;
    }
    if let Some(pos) = args.iter().position(|a| a == "--ax-rows") {
        match args.get(pos + 1).and_then(|pid| pid.parse::<u32>().ok()) {
            Some(pid) => {
                agent_monitor::dump_ax_rows(pid);
                return;
            }
            None => {
                eprintln!("usage: agent-monitor --ax-rows <pid>");
                return;
            }
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--jump") {
        if let Some(id) = args.get(pos + 1) {
            agent_monitor::jump_task(id);
            return;
        }
    }
    agent_monitor::run()
}
