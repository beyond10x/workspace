---
format: aep.planning-md/1
id: release-plan:workspace-0-2-18
kind: release-plan
status: active
title: Release Workspace 0.2.18 independently
relations:
- delivers: story:independent-runtime-publication
revision: 2
---
## Outcome

Publish Workspace 0.2.18 from its owner repository using the independent native-image pipeline.

## Source and evidence

Implementation commit 9f644b7 adds the owner workflow, release policy, real image smoke and retained-receipt recovery. Two adversarial passes resolved two introduced recovery findings. Final repository gate: all ten command steps exit 0; 42 runtime cases and 17 release cases pass. A local amd64 image built from the unchanged Dockerfile returned HTTP 200 for health and readiness.

## Procedure

Advance only Workspace's package version and matching local lock entries, run version and package verification, publish the bot-authored source to the provider default branch, then tag 0.2.18. Observe both native image lanes and immutable signed manifest publication. Verify successful same-source recovery performs no image build. Downstream consumers select the published digest separately.

## State

Source gate green; version change and remote image publication pending. Registry target is configured through repository administration, outside public source. No chart or Devcenter image participates.
