//! Binds angzarr-project's coordinator-contract
//! `features/coordinator-contract/policy_refusal.feature`, C-0617 only, to
//! the Rust core: the router's part of a refused policy command is the
//! class its answer's gRPC status decides (decision 14.37, owner rulings
//! of 2026-10-29). A business refusal reaches the policy's OnRefused (the
//! answer has a `RefusalResponse`); a bug is alerted and dead-lettered; the
//! rest are retried. Acting on the class (the retry, its cap, the dead
//! letter, the alert, building the refusal fact) is the runtime's.
//!
//! The router is domain-blind, so the order aggregate's AddItem stands in
//! for the wallet's HoldFunds: its handler answers as the scenario's wallet
//! does, and the router carries that answer.

use std::collections::BTreeMap;

use angzarr_router::error::{BusinessStatus, CodedError, ErrorClass, GrpcCode};
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

/// The wallet's refusal key, for a business-status answer.
const WALLET_REFUSAL: &str = "INSUFFICIENT_AVAILABLE_FUNDS";

#[derive(Debug, Default, World)]
struct RefusalWorld {
    /// The status the wallet answers HoldFunds with.
    status: Option<GrpcCode>,
    /// The wallet's answer to HoldFunds, as the router carries it.
    answer: Option<CodedError>,
}

impl RefusalWorld {
    fn answer(&self) -> &CodedError {
        self.answer.as_ref().expect("HoldFunds was answered")
    }
}

fn grpc_code(name: &str) -> GrpcCode {
    match name {
        "UNKNOWN" => GrpcCode::Unknown,
        "INVALID_ARGUMENT" => GrpcCode::InvalidArgument,
        "NOT_FOUND" => GrpcCode::NotFound,
        "ALREADY_EXISTS" => GrpcCode::AlreadyExists,
        "PERMISSION_DENIED" => GrpcCode::PermissionDenied,
        "FAILED_PRECONDITION" => GrpcCode::FailedPrecondition,
        "ABORTED" => GrpcCode::Aborted,
        "OUT_OF_RANGE" => GrpcCode::OutOfRange,
        "UNIMPLEMENTED" => GrpcCode::Unimplemented,
        "INTERNAL" => GrpcCode::Internal,
        "UNAVAILABLE" => GrpcCode::Unavailable,
        "DATA_LOSS" => GrpcCode::DataLoss,
        other => panic!("no gRPC status {other} the router carries"),
    }
}

// --- Background: the cast, which C-0617's router binding needs no more of ---

#[given(regex = r#"^a "([^"]*)" aggregate that refuses .*$"#)]
async fn an_aggregate(_w: &mut RefusalWorld, _domain: String) {
    // How the wallet refuses is the When step's answer, per example.
}

#[given(regex = r#"^a process manager "([^"]*)" owning domain "([^"]*)"$"#)]
async fn a_process_manager(_w: &mut RefusalWorld, _name: String, _domain: String) {
    // The source's coordinator acts on the class; the router does not.
}

#[given(regex = r#"^a policy "([^"]*)" translating from "([^"]*)" to "([^"]*)" that builds .*$"#)]
async fn a_policy(_w: &mut RefusalWorld, _name: String, _from: String, _to: String) {
    // The policy hears of a refusal only through its RefusalResponse.
}

// --- C-0617 -------------------------------------------------------------------

#[given(regex = r#"^wallet "([^"]*)" answers HoldFunds with (\w+)$"#)]
async fn wallet_answers(w: &mut RefusalWorld, _wallet: String, status: String) {
    w.status = Some(grpc_code(&status));
}

#[when(regex = r#"^"BuyIn" records BuyInStarted for (\d+) from wallet "([^"]*)"$"#)]
async fn buy_in_started(w: &mut RefusalWorld, amount: u64, wallet: String) {
    let status = w.status.expect("the wallet's answer is given");
    let fails = match BusinessStatus::try_from(status) {
        // A business status: the handler refuses under it.
        Ok(business) => Some(conf::AddItemFails::Rejects {
            status: business,
            code: WALLET_REFUSAL.to_string(),
            args: vec![("wallet".to_string(), wallet.clone())],
            logs: format!("cannot hold {amount}"),
        }),
        // INTERNAL: the handler's bug.
        Err(GrpcCode::Internal) => Some(conf::AddItemFails::Bug {
            code: "LEDGER_OFFLINE".to_string(),
            args: vec![("wallet".to_string(), wallet.clone())],
            logs: "the ledger is offline".to_string(),
        }),
        // Any other status is no handler's to answer (a handler refuses or
        // fails as a bug, by type): the transport answers it.
        Err(_) => None,
    };
    w.answer = Some(match fails {
        Some(fails) => conf::order_aggregate_with(
            "1.0.0",
            conf::Settle::default(),
            conf::OrderSink::default(),
            Some(fails),
        )
        .dispatch(&conf::order_command("AddItem", &wallet, &[], None, None))
        .expect_err("the wallet does not hold the funds"),
        None => CodedError {
            code: "WALLET_UNREACHABLE".to_string(),
            extras: BTreeMap::new(),
            grpc: status,
            refusal: None,
        },
    });
    assert_eq!(w.answer().grpc, status, "the router carries the status");
}

#[then(regex = r#"^"BuyInToWallet"'s OnRefused is called with the refusal$"#)]
async fn on_refused_called(w: &mut RefusalWorld) {
    let response = w
        .answer()
        .refusal_response()
        .unwrap_or_else(|| panic!("{:?} reaches OnRefused", w.answer().grpc));
    match response.reason {
        Some(angzarr_router::pb::refusal_response::Reason::Diagnostic(diagnostic)) => {
            assert_eq!(diagnostic.code, WALLET_REFUSAL, "the wallet's refusal key")
        }
        other => panic!("expected the wallet's refusal as its key and args, got {other:?}"),
    }
}

#[then(regex = r#"^"BuyInToWallet"'s OnRefused is not called$"#)]
async fn on_refused_not_called(w: &mut RefusalWorld) {
    assert_eq!(
        w.answer().refusal_response(),
        None,
        "{:?} never reaches OnRefused",
        w.answer().grpc
    );
}

#[then(
    regex = r"^HoldFunds is retried with backoff up to its configured cap, then dead-lettered and alerted$"
)]
async fn retried(w: &mut RefusalWorld) {
    assert_eq!(w.answer().grpc.class(), ErrorClass::Retryable);
}

#[then(regex = r"^HoldFunds is neither retried nor dead-lettered$")]
async fn neither(w: &mut RefusalWorld) {
    assert_eq!(w.answer().grpc.class(), ErrorClass::BusinessRefusal);
}

#[then(regex = r"^HoldFunds is dead-lettered and an alert is raised$")]
async fn dead_lettered(w: &mut RefusalWorld) {
    assert_eq!(w.answer().grpc.class(), ErrorClass::Bug);
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../angzarr-project/features/coordinator-contract/policy_refusal.feature"
    );
    RefusalWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(feature, |_feature, _rule, scenario| {
            scenario.tags.iter().any(|tag| tag == "C-0617")
        })
        .await;
}
