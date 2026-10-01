Feature: Facts, replay and handler context

  Beyond commands and events, an aggregate handles facts (external realities
  it may annotate but not refuse) and replays history into state; handlers
  read the cover they are handling; a process-manager compensator may issue
  commands; a projector fold knows where each event sits. The ledger
  aggregate, reserving process-manager and tracking projector are built
  through each binding's hand-written API over the fixture's CounterState.
  Roots are UUID v5 (NAMESPACE_OID) of the quoted label.

  Scenario: a fact handler annotates each fact against the folded state
    Given a ledger aggregate
    When 2 Increased facts are handled over 3 prior Increased events
    Then 2 facts are recorded, each annotated with a count of 3

  Scenario: a fact with no fact handler is recorded unchanged
    Given a ledger aggregate
    When a Reserve fact is handled over no prior events
    Then the fact is recorded unchanged

  Scenario: replay yields the state after a snapshot and later events
    Given a ledger aggregate
    When the ledger replays a snapshot of 10 then 2 Increased events
    Then the replayed state has a count of 12

  Scenario: a command handler reads its own cover
    Given a ledger aggregate
    When an IncreaseBy command for ledger root "ledger-1" is dispatched
    Then the ledger handler saw root "ledger-1"

  Scenario: a process-manager handler reads the trigger cover
    Given a reserving process-manager
    When an Increased trigger of counter root "table-1" at sequence 2 is dispatched to the reserving process-manager
    Then the reserving process-manager saw trigger root "table-1"

  Scenario: a process-manager compensator issues deferred commands
    Given a reserving process-manager
    When a rejection of Reserve sent to "inventory" at sequence 5 is dispatched to the reserving process-manager
    Then the reserving process-manager emits one Release command deferred from source sequence 5

  Scenario: a projector fold reads each event's root and sequence
    Given a tracking projector
    When Increased events of counter root "counter-1" at sequences 4 and 5 are projected
    Then the projector saw root "counter-1" at sequences 4 and 5
