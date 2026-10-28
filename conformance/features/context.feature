Feature: Facts and handler context

  Beyond commands, an aggregate handles facts: external realities delivered
  in the Fact arm of the command page, each recorded by the fact handler
  registered for its type, optionally followed by events that flag it, never
  refused. A fact whose type has no fact handler (a command type included:
  the arm must match the handler's kind) is refused with NO_FACT_HANDLER and
  nothing is recorded. Appliers see each event's sequence; handlers read the
  cover they are handling; a projector fold knows where each event sits. The
  ledger aggregate and tracking projector are
  built through each binding's hand-written API over the fixture's
  CounterState. Roots are UUID v5 (NAMESPACE_OID) of the quoted label.

  Scenario: a fact handler records the fact and flags it against the folded state
    Given a ledger aggregate
    When an Increased fact is handled over 3 prior Increased events
    Then the Increased fact is recorded, flagged by the count 4

  Scenario: a fact of an undeclared type is refused
    Given a ledger aggregate
    When a Reserve fact is handled over no prior events
    Then the fact is refused with NO_FACT_HANDLER as INVALID_ARGUMENT

  Scenario: a command delivered as a fact is refused
    Given a ledger aggregate
    When an IncreaseBy fact is handled over no prior events
    Then the fact is refused with NO_FACT_HANDLER as INVALID_ARGUMENT

  Scenario: an applier reads each event's sequence
    Given a ledger aggregate
    When an IncreaseBy command for ledger root "ledger-1" is dispatched over Increased events at sequences 2 and 3
    Then the ledger applied Increased events at sequences 2 and 3, then its emitted event at 4

  Scenario: a command handler reads its own cover
    Given a ledger aggregate
    When an IncreaseBy command for ledger root "ledger-1" is dispatched
    Then the ledger handler saw root "ledger-1"

  Scenario: a projector fold reads each event's root and sequence
    Given a tracking projector
    When Increased events of counter root "counter-1" at sequences 4 and 5 are projected
    Then the projector saw root "counter-1" at sequences 4 and 5
