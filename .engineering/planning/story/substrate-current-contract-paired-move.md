---
format: aep.planning-md/3
id: story:substrate-current-contract-paired-move
kind: story
status: draft
title: Move Workspace onto the current Substrate contract as one paired step
summary: The SDK pin (Substrate 0.7.3, wire 0.16.0) cannot use a 0.24.0 daemon (wire 0.27.0); the pin and the daemon move together
revision: 1
---
## Outcome

Workspace's Substrate SDK pin and the Substrate daemon a deployment runs name one Substrate release,
the current one, and they move in one step. No daemon is upgraded alone.

## What was seen

- read: `Cargo.toml:25` pins `b10x-substrate-sdk` at Substrate revision
  `329a128606a7c18c4477d6fff58ffc57b296fb70`, `=0.7.3` (Workspace 0.2.25, `main` at `0ec0e04`).
- read: at that revision Substrate advertises `substrate-wire/0.16.0`
  (`crates/substrate-wire/src/lib.rs:18` in `beyond10x/substrate`). Substrate 0.24.0 (commit
  `85da2e2be46b690993007990905d6d3934650774`) advertises `substrate-wire/0.27.0` on the same line.
- read: the pinned SDK refuses a response whose `x-b10x-contract` or
  `x-b10x-contract-bundle-sha256` header is not its own (`verify_contract_headers`,
  `crates/b10x-substrate-sdk/src/transport.rs:802` at the pinned revision). The name and the digest
  must both match, so Workspace 0.2.25 cannot use a 0.24.0 daemon.
- run by the review, not repeated here: the released 0.24.0 daemon answered with
  `substrate-wire/0.27.0`.
- read: `README.md:126` tells a developer to build `substrate-daemon` from the exact Substrate
  revision this repository pins, and `README.md:48` lets a hosted deployment configure the released
  Substrate remote client. Both work only while the daemon is at the pinned contract.
- read: no item in this store moves the pin.

## What changes

The SDK pin moves to the Substrate release a deployment's daemon runs, the call sites the SDK
changed between `substrate-wire/0.16.0` and `0.27.0` are adapted, and the README names that
release. In which order a deployment's daemon and Workspace are replaced is the owner's decision.

## Acceptance

Workspace's transport regression passes the SDK handshake against a Substrate 0.24.0 daemon, and
the README names the Substrate release the pin is.

## Where the requirement comes from

A review of the Substrate release 0.24.0 dated 2026-11-23, finding 9, which the operator handed to
the substrate session on 2026-11-23 with the order to address every finding. Filed by the substrate
session. Stakeholder: the operator.
