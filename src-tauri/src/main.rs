#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
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
    if let Some(pos) = args.iter().position(|a| a == "--jump") {
        if let Some(id) = args.get(pos + 1) {
            agent_monitor::jump_task(id);
            return;
        }
    }
    agent_monitor::run()
}
