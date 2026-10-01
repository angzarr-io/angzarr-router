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

  The payment and inventory aggregates are built through each binding's
  hand-written aggregate API.

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
