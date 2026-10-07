//go:build ffirouter

package conformance

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// TestProcessManagerConformance runs the shared process_manager.feature
// behavior suite against the Go binding via godog — the same feature the Rust
// cucumber-rs harness drives against the core. Only the step layer is new.
func TestProcessManagerConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeProcessManagerScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../../conformance/features/process_manager.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("process-manager conformance scenarios failed")
	}
}

type pmWorld struct {
	router *Router
	seen   rejectionSink
	resp   *pb.ProcessManagerHandleResponse
	err    error
}

func (w *pmWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.seen = nil
	w.resp = nil
	w.err = nil
}

// --- Given ---

func (w *pmWorld) orderPM() error {
	if err := counter.RegisterOrderProcessManager(w.router, orderPM{seen: &w.seen}); err != nil {
		return fmt.Errorf("register order PM fixture: %w", err)
	}
	return nil
}

func (w *pmWorld) coResidentPMs() error {
	if err := w.orderPM(); err != nil {
		return err
	}
	if err := counter.RegisterAuditProcessManager(w.router, auditPM{}); err != nil {
		return fmt.Errorf("register audit PM fixture: %w", err)
	}
	return nil
}

func (w *pmWorld) dispatch(req *pb.ProcessManagerHandleRequest) {
	w.resp, w.err = w.router.DispatchProcessManager(req)
}

// pmTrigger is a request whose trigger carries the given event pages in
// domain (the newest at sequence seq when non-nil), plus the PM's prior state.
func pmTrigger(domain string, fqs []string, state *pb.EventBook, seq *uint32) *pb.ProcessManagerHandleRequest {
	pages := make([]*pb.EventPage, len(fqs))
	for i, fq := range fqs {
		pages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: &anypb.Any{TypeUrl: typeURL(fq)}}}
	}
	if seq != nil && len(pages) > 0 {
		pages[len(pages)-1].Header = sequenceHeader(*seq)
	}
	return &pb.ProcessManagerHandleRequest{
		Trigger:      &pb.EventBook{Cover: &pb.Cover{Domain: domain}, Pages: pages},
		ProcessState: state,
	}
}

// pmStateOf is a prior-state book of n Increased events (drives the rebuild).
func pmStateOf(n int) *pb.EventBook {
	pages := make([]*pb.EventPage, n)
	for i := range pages {
		pages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: increasedAny()}}
	}
	return &pb.EventBook{Pages: pages}
}

// pmRejection is a trigger whose newest page is a rejection Notification for
// fqCommand.
func pmRejection(fqCommand string) *pb.ProcessManagerHandleRequest {
	return pmRejectionWith(fqCommand, "", "")
}

// pmRejectionWith is pmRejection whose rejection carries code and message.
func pmRejectionWith(fqCommand, code, message string) *pb.ProcessManagerHandleRequest {
	rejection := &pb.RejectionNotification{
		RejectedCommand: &pb.CommandBook{
			Cover: &pb.Cover{Domain: "inventory"},
			Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
				Command: &anypb.Any{TypeUrl: typeURL(fqCommand)},
			}}},
		},
		RejectionReason: message,
		Code:            code,
	}
	return pmRejectionOf(rejection)
}

// pmRejectionOf wraps a RejectionNotification as the newest trigger page of a
// "counter"-covered rejection request.
func pmRejectionOf(rejection *pb.RejectionNotification) *pb.ProcessManagerHandleRequest {
	notification := &pb.Notification{
		Payload: &anypb.Any{
			TypeUrl: typeURL("io.angzarr.v1.RejectionNotification"),
			Value:   mustMarshal(rejection),
		},
	}
	return &pb.ProcessManagerHandleRequest{
		Trigger: &pb.EventBook{
			Cover: &pb.Cover{Domain: "counter"},
			Pages: []*pb.EventPage{{Payload: &pb.EventPage_Event{Event: &anypb.Any{
				TypeUrl: typeURL("io.angzarr.v1.Notification"),
				Value:   mustMarshal(notification),
			}}}},
		},
	}
}

// pmIssuedRejection is a rejection of fqCommand issued by the PM owning
// issuer: the trigger cover is the issuer's domain and the rejected command's
// first page carries an angzarr_deferred header naming the issuer as source.
func pmIssuedRejection(fqCommand, issuer string) *pb.ProcessManagerHandleRequest {
	rejection := &pb.RejectionNotification{
		RejectedCommand: &pb.CommandBook{
			Cover: &pb.Cover{Domain: "inventory"},
			Pages: []*pb.CommandPage{{
				Header: &pb.PageHeader{SequenceType: &pb.PageHeader_AngzarrDeferred{
					AngzarrDeferred: &pb.AngzarrDeferredSequence{Source: &pb.Cover{Domain: issuer}},
				}},
				Payload: &pb.CommandPage_Command{Command: &anypb.Any{TypeUrl: typeURL(fqCommand)}},
			}},
		},
	}
	req := pmRejectionOf(rejection)
	req.Trigger.Cover = &pb.Cover{Domain: issuer}
	return req
}

// pmCompensate is a request whose trigger, in the order PM's own domain, is a
// Compensate Notification for an executed fqCommand.
func pmCompensate(fqCommand string) *pb.ProcessManagerHandleRequest {
	notification := &pb.Notification{Payload: compensatePayload(fqCommand)}
	return &pb.ProcessManagerHandleRequest{
		Trigger: &pb.EventBook{
			Cover: &pb.Cover{Domain: "order-pm"},
			Pages: []*pb.EventPage{{Payload: &pb.EventPage_Event{Event: &anypb.Any{
				TypeUrl: typeURL("io.angzarr.v1.Notification"),
				Value:   mustMarshal(notification),
			}}}},
		},
	}
}

// --- When ---

func (w *pmWorld) increasedAt(domain string, seq int) {
	s := uint32(seq)
	w.dispatch(pmTrigger(domain, []string{fqIncreased}, nil, &s))
}

func (w *pmWorld) compensateReserve() {
	w.dispatch(pmCompensate(fqReserve))
}

func (w *pmWorld) increasedInDomain(domain string) {
	w.dispatch(pmTrigger(domain, []string{fqIncreased}, nil, nil))
}

func (w *pmWorld) newestUndeclared() {
	w.dispatch(pmTrigger("counter", []string{fqIncreased, "test.counter.Unwatched"}, nil, nil))
}

func (w *pmWorld) increasedOverState(n int) {
	w.dispatch(pmTrigger("counter", []string{fqIncreased}, pmStateOf(n), nil))
}

func (w *pmWorld) noTrigger() {
	w.dispatch(&pb.ProcessManagerHandleRequest{})
}

func (w *pmWorld) emptyTrigger() {
	w.dispatch(&pb.ProcessManagerHandleRequest{Trigger: &pb.EventBook{}})
}

func (w *pmWorld) rejectionReserve() {
	w.dispatch(pmRejection(fqReserve))
}

func (w *pmWorld) increasedOverOwnedState(owner string, n int) {
	state := pmStateOf(n)
	state.Cover = &pb.Cover{Domain: owner}
	w.dispatch(pmTrigger("counter", []string{fqIncreased}, state, nil))
}

func (w *pmWorld) rejectionWithCode(code, message string) {
	w.dispatch(pmRejectionWith(fqReserve, code, message))
}

func (w *pmWorld) compensatorSawCode(code, message string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	return w.seen.exactly(code, message)
}

func (w *pmWorld) rejectionIssuedBy(issuer string) {
	w.dispatch(pmIssuedRejection(fqReserve, issuer))
}

// --- Then ---

func (w *pmWorld) emitsOneCommand(target string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetCommands()); got != 1 {
		return fmt.Errorf("emitted %d commands, want 1", got)
	}
	if domain := w.resp.GetCommands()[0].GetCover().GetDomain(); domain != target {
		return fmt.Errorf("command targets %q, want %q", domain, target)
	}
	return nil
}

func (w *pmWorld) commandIsDeferred(seq, index int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	return assertDeferred(w.resp.GetCommands()[0], "counter", seq, index)
}

func (w *pmWorld) leavesSourceComponent() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if len(w.resp.GetCommands()) == 0 {
		return errors.New("no command emitted")
	}
	return assertNoSourceComponent(w.resp.GetCommands()[0])
}

func (w *pmWorld) emitsNoCommands() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetCommands()); got != 0 {
		return fmt.Errorf("emitted %d commands, want 0", got)
	}
	return nil
}

func (w *pmWorld) rebuiltN(n int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetFacts()); got != n {
		return fmt.Errorf("rebuilt %d prior state events, want %d", got, n)
	}
	return nil
}

// factsMarked counts the response facts whose cover domain is (audit=true) or
// is not (audit=false) the audit PM's mark.
func (w *pmWorld) factsMarked(audit bool) int {
	n := 0
	for _, f := range w.resp.GetFacts() {
		if (f.GetCover().GetDomain() == auditMark) == audit {
			n++
		}
	}
	return n
}

func (w *pmWorld) orderRebuiltN(n int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := w.factsMarked(false); got != n {
		return fmt.Errorf("order PM rebuilt %d prior state events, want %d", got, n)
	}
	return nil
}

func (w *pmWorld) auditRebuiltN(n int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := w.factsMarked(true); got != n {
		return fmt.Errorf("audit PM rebuilt %d prior state events, want %d", got, n)
	}
	return nil
}

func (w *pmWorld) orderDidNotReact() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetCommands()); got != 0 {
		return fmt.Errorf("order PM emitted %d commands, want 0", got)
	}
	if got := w.factsMarked(false); got != 0 {
		return fmt.Errorf("order PM emitted %d facts, want 0", got)
	}
	return nil
}

func (w *pmWorld) onlyAuditCompensates() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	events := w.resp.GetProcessEvents()
	if len(events) != 1 {
		return fmt.Errorf("emitted %d process events, want exactly 1", len(events))
	}
	if domain := events[0].GetCover().GetDomain(); domain != auditMark {
		return fmt.Errorf("process event cover %q, want %q (the audit PM)", domain, auditMark)
	}
	if w.resp.GetNotification() != nil {
		return fmt.Errorf("order PM escalated; only the audit PM should compensate")
	}
	return nil
}

func (w *pmWorld) processEventAddressedTo(domain string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	events := w.resp.GetProcessEvents()
	if len(events) != 1 {
		return fmt.Errorf("emitted %d process events, want exactly 1", len(events))
	}
	if got := events[0].GetCover().GetDomain(); got != domain {
		return fmt.Errorf("process event cover domain %q, want %q", got, domain)
	}
	return nil
}

func (w *pmWorld) emitsOneProcessEvent() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetProcessEvents()); got != 1 {
		return fmt.Errorf("emitted %d process events, want 1", got)
	}
	return nil
}

func (w *pmWorld) escalates() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if w.resp.GetNotification() == nil {
		return fmt.Errorf("expected an escalation, got none")
	}
	return nil
}

func (w *pmWorld) failsWith(code string) error {
	var ce *CodedError
	if !errors.As(w.err, &ce) {
		return fmt.Errorf("expected coded error %s, got %v", code, w.err)
	}
	if ce.Code != code {
		return fmt.Errorf("expected %s, got %s", code, ce.Code)
	}
	return nil
}

func initializeProcessManagerScenario(sc *godog.ScenarioContext) {
	w := &pmWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^an order process-manager$`, w.orderPM)
	sc.Step(`^co-resident order and audit process-managers$`, w.coResidentPMs)
	sc.Step(`^an Increased trigger is dispatched over a prior "([^"]*)" state of (\d+) events$`, w.increasedOverOwnedState)
	sc.Step(`^a rejection of Reserve issued by "([^"]*)" is dispatched$`, w.rejectionIssuedBy)
	sc.Step(`^the order process-manager rebuilt (\d+) prior state events$`, w.orderRebuiltN)
	sc.Step(`^the audit process-manager rebuilt (\d+) prior state events$`, w.auditRebuiltN)
	sc.Step(`^the order process-manager did not react$`, w.orderDidNotReact)
	sc.Step(`^only the audit process-manager compensates$`, w.onlyAuditCompensates)
	sc.Step(`^an Increased trigger in domain "([^"]*)" at sequence (\d+) is dispatched$`, w.increasedAt)
	sc.Step(`^a Compensate for Reserve is dispatched to the order process-manager$`, w.compensateReserve)
	sc.Step(`^an Increased trigger in domain "([^"]*)" is dispatched$`, w.increasedInDomain)
	sc.Step(`^a trigger whose newest page is an undeclared event is dispatched$`, w.newestUndeclared)
	sc.Step(`^an Increased trigger is dispatched over a prior state of (\d+) events$`, w.increasedOverState)
	sc.Step(`^a request with no trigger is dispatched$`, w.noTrigger)
	sc.Step(`^a trigger with no pages is dispatched$`, w.emptyTrigger)
	sc.Step(`^a rejection of Reserve is dispatched$`, w.rejectionReserve)
	sc.Step(`^a rejection of Reserve with code "([^"]*)" and message "([^"]*)" is dispatched$`, w.rejectionWithCode)
	sc.Step(`^the process-manager compensator saw code "([^"]*)" and message "([^"]*)"$`, w.compensatorSawCode)
	sc.Step(`^the process-manager emits one command to "([^"]*)"$`, w.emitsOneCommand)
	sc.Step(`^the command is deferred from source sequence (\d+) at command index (\d+)$`, w.commandIsDeferred)
	sc.Step(`^the command leaves its source component to the coordinator$`, w.leavesSourceComponent)
	sc.Step(`^the process-manager emits no commands$`, w.emitsNoCommands)
	sc.Step(`^the process-manager rebuilt (\d+) prior state events$`, w.rebuiltN)
	sc.Step(`^the process-manager emits one process event$`, w.emitsOneProcessEvent)
	sc.Step(`^the process-manager escalates$`, w.escalates)
	sc.Step(`^the process event is addressed to "([^"]*)"$`, w.processEventAddressedTo)
	sc.Step(`^the dispatch fails with ([A-Z_]+)$`, w.failsWith)
}
