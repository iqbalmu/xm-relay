use xm_relay_core::AgentId;

fn main() {
    println!("xm-relay {}", env!("CARGO_PKG_VERSION"));
    println!("planned agents:");
    for a in AgentId::ALL {
        println!("  - {}", a.as_str());
    }
}
