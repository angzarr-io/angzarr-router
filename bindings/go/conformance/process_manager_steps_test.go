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
	resp   *pb.ProcessManagerHandleResponse
	err    error
}

func (w *pmWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.resp = nil
	w.err = nil
	if err := counter.RegisterOrderProcessManager(w.router, orderPM{}); err != nil {
		panic(fmt.Sprintf("register PM fixture: %v", err))
	}
}

func (w *pmWorld) dispatch(req *pb.ProcessManagerHandleRequest) {
	w.resp, w.err = w.router.DispatchProcessManager(req)
}

// pmTrigger is a request whose trigger carries the given event pages in
// domain, plus the PM's prior state and a destination map.
func pmTrigger(domain string, fqs []string, state *pb.EventBook, dest map[string]uint32) *pb.ProcessManagerHandleRequest {
	pages := make([]*pb.EventPage, len(fqs))
	for i, fq := range fqs {
		pages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: &anypb.Any{TypeUrl: typeURL(fq)}}}
	}
	return &pb.ProcessManagerHandleRequest{
		Trigger:              &pb.EventBook{Cover: &pb.Cover{Domain: domain}, Pages: pages},
		ProcessState:         state,
		DestinationSequences: dest,
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
	rejection := &pb.RejectionNotification{
		RejectedCommand: &pb.CommandBook{
			Cover: &pb.Cover{Domain: "inventory"},
			Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
				Command: &anypb.Any{TypeUrl: typeURL(fqCommand)},
			}}},
		},
	}
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

// --- When ---

func (w *pmWorld) increasedWithDestination(domain string, seq int) {
	w.dispatch(pmTrigger(domain, []string{fqIncreased}, nil, map[string]uint32{"inventory": uint32(seq)}))
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

func (w *pmWorld) commandCarriesSequence(seq int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	got := w.resp.GetCommands()[0].GetPages()[0].GetHeader().GetSequence()
	if got != uint32(seq) {
		return fmt.Errorf("command carries sequence %d, want %d", got, seq)
	}
	return nil
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

	sc.Step(`^an order process-manager$`, func() {})
	sc.Step(`^an Increased trigger in domain "([^"]*)" is dispatched with destination inventory sequence (\d+)$`, w.increasedWithDestination)
	sc.Step(`^an Increased trigger in domain "([^"]*)" is dispatched$`, w.increasedInDomain)
	sc.Step(`^a trigger whose newest page is an undeclared event is dispatched$`, w.newestUndeclared)
	sc.Step(`^an Increased trigger is dispatched over a prior state of (\d+) events$`, w.increasedOverState)
	sc.Step(`^a request with no trigger is dispatched$`, w.noTrigger)
	sc.Step(`^a trigger with no pages is dispatched$`, w.emptyTrigger)
	sc.Step(`^a rejection of Reserve is dispatched$`, w.rejectionReserve)
	sc.Step(`^the process-manager emits one command to "([^"]*)"$`, w.emitsOneCommand)
	sc.Step(`^the command carries destination sequence (\d+)$`, w.commandCarriesSequence)
	sc.Step(`^the process-manager emits no commands$`, w.emitsNoCommands)
	sc.Step(`^the process-manager rebuilt (\d+) prior state events$`, w.rebuiltN)
	sc.Step(`^the process-manager emits one process event$`, w.emitsOneProcessEvent)
	sc.Step(`^the process-manager escalates$`, w.escalates)
	sc.Step(`^the dispatch fails with ([A-Z_]+)$`, w.failsWith)
}
