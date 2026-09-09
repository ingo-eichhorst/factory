//! Runs backlog §10's contract suite against [`PiAdapter`], and adds one
//! Pi-specific test the generic suite deliberately does not attempt: proving
//! a failed `agent_start` does not leak the pane `start` created for it.
//!
//! `factory_adapter::contract::run_contract_suite` is written once and
//! exported so a future Claude Code adapter (Slice 10) runs the identical
//! assertions through its own fixture, rather than a hand-copied variant of
//! this file. See the `contract` module's own doc comment for why.

mod support;

use factory_adapter::Adapter;
use factory_adapter::contract::run_contract_suite;
use support::PiFixture;

#[test]
fn pi_adapter_satisfies_the_full_contract_suite() {
    let fixture = PiFixture::new();
    run_contract_suite(&fixture);
}

#[test]
fn a_failed_agent_start_closes_the_pane_start_created() {
    use factory_adapter::contract::ContractFixture;

    let fixture = PiFixture::new();
    fixture.fail_next_agent_start();

    let req = fixture.startable();
    fixture
        .adapter()
        .start(&req)
        .expect_err("a fixture configured to fail agent_start must fail start");

    assert_eq!(
        fixture.pane_close_calls().len(),
        1,
        "the pane created before agent_start failed must be closed exactly once, or it leaks"
    );
}
