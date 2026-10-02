Feature: Compensation routing

  A Notification delivered to an aggregate carries one of two payloads. A
  RejectionNotification (a command the aggregate issued was rejected) routes
  to the compensation handlers its `compensates` entries declare: an entry
  names the rejected command's fully-qualified type, optionally qualified by
  the domain the command was sent to ("domain:fq.Type"). A Compensate (a
  command the aggregate executed must be undone) routes to the undo handler
  its `undoes` declares for the Compensate's command type; with no such
  handler the dispatch is refused as UNIMPLEMENTED so the coordinator
  dead-letters it — never silently dropped. Events a compensation or undo
  handler returns append after the aggregate's prior history.

  A compensation handler receives the rejection whole: its `code` (the
  rejecting handler's machine code, empty when it gave none) and its
  `rejection_reason` (the human-readable message) are separate fields, and
  handlers branch on the code.

  Several aggregates may share a domain. Every one of them whose
  `compensates` entries match a rejection runs, in registration order; their
  events concatenate into one book whose sequences continue after the
  domain's prior history.

  The payment and inventory aggregates are built through each binding's
  hand-written aggregate API and dispatched through its router.

  Scenario: an unqualified compensates entry matches a rejection sent to any domain
    Given a payment aggregate compensating Reserve from any domain with FundsReleased
    When a rejection of Reserve sent to "warehouse" is dispatched to the payment aggregate
    Then the aggregate emits one FundsReleased event

  Scenario: a domain-qualified compensates entry matches only rejections sent to its domain
    Given a payment aggregate compensating Reserve from "inventory" with FundsReleased and from "warehouse" with WorkflowFailed
    When a rejection of Reserve sent to "warehouse" is dispatched to the payment aggregate
    Then the aggregate emits one WorkflowFailed event

  Scenario: a rejection matching no compensates entry is left to the framework
    Given a payment aggregate compensating Reserve from "inventory" with FundsReleased and from "warehouse" with WorkflowFailed
    When a rejection of Reserve sent to "billing" is dispatched to the payment aggregate
    Then the aggregate emits nothing

  Scenario: compensation events append after prior history
    Given a payment aggregate compensating Reserve from any domain with FundsReleased
    When a rejection of Reserve sent to "inventory" is dispatched to the payment aggregate over history ending at sequence 6
    Then the emitted event takes sequence 7

  Scenario: a Compensate routes to the undo handler for its command type
    Given an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased
    When a Compensate for AdjustStock is dispatched to the inventory aggregate
    Then the aggregate emits one StockAdjustmentReverted event

  Scenario: a Compensate for a command with no undo handler is refused as UNIMPLEMENTED
    Given an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased
    When a Compensate for CountStock is dispatched to the inventory aggregate
    Then the dispatch fails with NO_UNDO_HANDLER as UNIMPLEMENTED

  Scenario: every aggregate of a domain that compensates a rejection runs in registration order
    Given a payment aggregate compensating Reserve from any domain with FundsReleased
    And a second payment aggregate compensating Reserve from any domain with WorkflowFailed
    When a rejection of Reserve sent to "inventory" is dispatched to the payment aggregate over history ending at sequence 6
    Then the aggregates emit FundsReleased at sequence 7 then WorkflowFailed at sequence 8

  Scenario: a compensation handler reads the rejection's code and message separately
    Given a payment aggregate compensating Reserve from any domain with FundsReleased
    When a rejection of Reserve with code "OUT_OF_STOCK" and message "no stock left" is dispatched to the payment aggregate
    Then the compensation handler saw code "OUT_OF_STOCK" and message "no stock left"

  Scenario: a rejection without a code reaches the compensation handler with an empty code
    Given a payment aggregate compensating Reserve from any domain with FundsReleased
    When a rejection of Reserve with no code and message "no stock left" is dispatched to the payment aggregate
    Then the compensation handler saw an empty code and message "no stock left"
