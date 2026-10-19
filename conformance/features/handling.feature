Feature: Rules version, conflict resolution and command upcasting

  What every implementation of CommandHandlerService.Handle does around
  business code: the payload is upcast through the component's upcaster
  chain, the body arm and type URL pick the handler, AGGREGATE_HANDLES and
  COMMUTATIVE are resolved through the conflict nonce, and every response
  reports the component's rules version. (Snapshot validity is the
  coordinator's: it chooses which snapshot to offer from the business
  binary's startup hello.)

  Spec (angzarr-project spec/20261019-draft): command_handler.proto
  CommandHandlerService.Handle and CommandHandlerResponse; types.proto
  Handling, ConflictResolution and ContextualCommand; options.proto
  conflict_units region; upcaster.proto. Decisions 12.6, 12.7 and 12.11;
  backlog RV-2, HD-5, NC-1, CR-1 and VC-1. The project features carry no
  scenario ids for these yet.

  The list aggregate (domain "list") keeps one counter per item key and
  declares the conflict unit items[<key>]. Bump(key) records Bumped(key).
  BumpV1 is Bump's old name and BumpOld upcasts to BumpLost, which nothing
  handles. Its rules version is "1.4.2+build.7".

  Background:
    Given a list aggregate at rules version "1.4.2+build.7"

  Scenario: every response reports the rules version
    When Bump "a" is handled over no prior events
    Then the response reports rules version "1.4.2+build.7"

  Scenario: under STRICT the handler sees its handling and no conflict
    When Bump "a" is handled STRICT at expected sequence 2 over bumps of "x" and "y"
    Then the handler saw expected sequence 2 and resolution STRICT
    And the handler saw no conflict window

  Scenario: under AGGREGATE_HANDLES the handler receives the window and the nonce
    Given the handler resolves conflicts with the delivered nonce
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 1 over bumps of "x", "y" and "z" with nonce "n-1"
    Then the handler saw the window at sequences 1 and 2
    And the command is accepted

  Scenario: under AGGREGATE_HANDLES a handler that ignores the window fails stale
    Given the handler does not resolve conflicts
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 0 over bumps of "x" with nonce "n-1"
    Then the command fails with CONFLICT_UNRESOLVED as ABORTED
    And no events are recorded

  Scenario: under AGGREGATE_HANDLES a made-up nonce fails stale
    Given the handler resolves conflicts with nonce "made-up"
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 0 over bumps of "x" with nonce "n-1"
    Then the command fails with CONFLICT_UNRESOLVED as ABORTED

  Scenario: under AGGREGATE_HANDLES an empty window still demands the nonce
    Given the handler does not resolve conflicts
    When Bump "a" is handled AGGREGATE_HANDLES at expected sequence 1 over bumps of "x" with nonce "n-1"
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
    Then the command is accepted
    And the handler saw a Bump

  Scenario: an upcast type no handler claims is refused naming both types
    When BumpOld "a" is handled over no prior events
    Then the command is refused INVALID_ARGUMENT naming the type URL "/test.counter.BumpOld"
    And the refusal carries the upcast type URL "/test.counter.BumpLost"

  Scenario: a type with no handler and no upcaster path is refused naming the sent type
    When BumpV0 "a" is handled over no prior events
    Then the command is refused INVALID_ARGUMENT naming the type URL "/test.counter.BumpV0"
