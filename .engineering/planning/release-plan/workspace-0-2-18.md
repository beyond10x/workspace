---
format: aep.planning-md/1
id: release-plan:workspace-0-2-18
kind: release-plan
status: implemented
title: Release Workspace 0.2.18 independently
relations:
- delivers: story:independent-runtime-publication
revision: 4
---
## Outcome

Publish Workspace 0.2.18 from its owner repository using the independent native-image pipeline.

## Source and evidence

Implementation commit 9f644b7 adds the owner workflow, release policy, real image smoke and retained-receipt recovery. Two adversarial passes resolved two introduced recovery findings. Final repository gate: all ten command steps exit 0; 42 runtime cases and 17 release cases pass. A local amd64 image built from the unchanged Dockerfile returned HTTP 200 for health and readiness.

## Procedure

Advance only Workspace's package version and matching local lock entries, run version and package verification, publish the bot-authored source to the provider default branch, then tag 0.2.18. Observe both native image lanes and immutable signed manifest publication. Verify successful same-source recovery performs no image build. Downstream consumers select the published digest separately.

## State

Published Workspace 0.2.18 from exact source 2cb2d07c73e6bebe7623aaeb84d143089d392e9f. Owner run 33935985625 succeeded after the strict pre-existing private-target correction 53051b4 and administrative provisioning of a distinct private target. Both native images passed health/readiness smoke before push; publication signed and verified the immutable index and confirmed privacy again.

The downloaded release-manifest.json identifies index sha256:128b78076ba9833f955a32a9e2238ba48f8f5e96658e99fd4fef4f851e479c96, linux/amd64 sha256:cdaa23648752bc8df2b2796ed2b21461050b373a2d2d820f9c377082992829ed, and linux/arm64 sha256:a240bbd4153c8b47c6209a8de3e0852154610f8c7768c464d5a1103fea121b13. Public metadata contains no registry coordinate.

Same-source retry 33936402567 succeeded with images and publish jobs skipped. This verifies the published receipt, registry artifacts and owner signature without rebuilding or replacing version identity. Downstream deployment selection remains separate.
