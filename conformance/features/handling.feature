@wip
Feature: Rules version, snapshot validity, conflict resolution and command upcasting

  DESIGN SCENARIOS, pending sign-off of docs/design-handling.md: no runner
  drives this file yet (crates/conformance wires it in phase b). They pin
  what every implementation of CommandHandlerService.Handle does before and
  around business code, in the spec's order:
    1. a delivered snapshot is validated before it is decoded: its state
       type and rules version against the component's own, else the
       response is snapshot_refused and no business code runs;
    2. the payload is upcast through the component's upcaster chain;
    3. the body arm and type URL pick the handler;
    4. AGGREGATE_HANDLES and COMMUTATIVE are resolved through the conflict
       nonce.
  Every response reports the component's rules version.

  Spec (angzarr-project spec/20261019-draft): command_handler.proto
  CommandHandlerService.Handle and CommandHandlerResponse; types.proto
  Snapshot, Handling, ConflictResolution and ContextualCommand; options.proto
  conflict_units region; upcaster.proto. Decisions 12.6, 12.7 and 12.11;
  backlog RV-2, RV-3, HD-5, NC-1, CR-1 and VC-1. The project features carry
  no scenario ids for these yet.

  The list aggregate (domain "list") keeps one counter per item key. Its
  state type is test.ListState, its rules version "v3", and it declares the
  conflict unit items[<key>]. Bump(key) bumps one item; BumpV1 is its old
  name, upcast to Bump.

  Background:
    Given a list aggregate at rules version "v3" with state type "test.ListState"

  Scenario: every response reports the rules version
    When Bump "a" is handled over no prior events
    Then the response reports rules version "v3"

  Scenario: a snapshot of another state type is refused before business code
    When Bump "a" is handled over a snapshot of type "test.OtherState" at rules version "v3"
    Then the snapshot is refused
    And no business code ran

  Scenario: a snapshot of another rules version is refused before business code
    When Bump "a" is handled over a snapshot of type "test.ListState" at rules version "v2"
    Then the snapshot is refused
    And no business code ran

  Scenario: a snapshot of the component's own state type and rules version loads
    When Bump "a" is handled over a snapshot of type "test.ListState" at rules version "v3"
    Then the command is accepted

  Scenario: under STRICT the handler sees its handling and no conflict
    When Bump "a" is handled STRICT at expected sequence 2 over bumps of "x" and "y"
    Then the handler saw expected sequence 2 and resolution STRICT
    And the handler saw no conflict window

  Scenario: under AGGREGATE_HANDLES the handler receives the window and the nonce
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 1 over bumps of "x", "y" and "z" with nonce "n-1"
    And the handler resolves conflicts with the delivered nonce
    Then the handler saw the window at sequences 1 and 2
    And the command is accepted

  Scenario: under AGGREGATE_HANDLES a handler that ignores the window fails stale
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 0 over bumps of "x" with nonce "n-1"
    And the handler does not resolve conflicts
    Then the command fails with CONFLICT_UNRESOLVED as ABORTED
    And no events are recorded

  Scenario: under AGGREGATE_HANDLES a made-up nonce fails stale
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 0 over bumps of "x" with nonce "n-1"
    And the handler resolves conflicts with nonce "made-up"
    Then the command fails with CONFLICT_UNRESOLVED as ABORTED

  Scenario: under AGGREGATE_HANDLES an empty window still demands the nonce
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 1 over bumps of "x" with nonce "n-1"
    And the handler does not resolve conflicts
    Then the command fails with CONFLICT_UNRESOLVED as ABORTED

  Scenario: COMMUTATIVE keeps a stale command disjoint from the window
    When Bump "a" is handled COMMUTATIVE at expected sequence 0 over bumps of "b" and "b" with nonce "n-1"
    Then the command is accepted
    And the handler saw no conflict window

  Scenario: COMMUTATIVE fails a stale command that overlaps the window
    When Bump "b" is handled COMMUTATIVE at expected sequence 1 over bumps of "a" and "b" with nonce "n-1"
    Then the command fails with CONFLICT_OVERLAP as ABORTED
    And no events are recorded

  Scenario: COMMUTATIVE counts only events from the expected sequence
    When Bump "b" is handled COMMUTATIVE at expected sequence 1 over bumps of "b" and "a" with nonce "n-1"
    Then the command is accepted

  Scenario: COMMUTATIVE decides on the head
    When Bump "a" is handled COMMUTATIVE at expected sequence 0 over bumps of "b" and "c" with nonce "n-1"
    Then the handler saw items "b" and "c" in its state

  Scenario: an old command type is upcast and handled as the new type
    When BumpV1 "a" is handled over no prior events
    Then the Bump handler ran

  Scenario: a type with no handler and no upcaster path is refused naming the sent type
    When BumpV0 "a" is handled over no prior events
    Then the command is refused INVALID_ARGUMENT naming the type URL "/test.BumpV0"
