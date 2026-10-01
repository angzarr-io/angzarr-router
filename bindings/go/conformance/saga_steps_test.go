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

// TestSagaConformance runs the shared saga.feature behavior suite against the
// Go binding via godog — the same feature the Rust cucumber-rs harness drives
// against the core. Only the step layer is new.
func TestSagaConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeSagaScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../../conformance/features/saga.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("saga conformance scenarios failed")
	}
}

// sagaWorld holds one scenario's state: a router with the saga fixture
// registered, and the dispatch outcome.
type sagaWorld struct {
	router *Router
	resp   *pb.SagaResponse
	err    error
}

func (w *sagaWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.resp = nil
	w.err = nil
	if err := counter.RegisterOrderSaga(w.router, orderSaga{}); err != nil {
		panic(fmt.Sprintf("register saga fixture: %v", err))
	}
}

func (w *sagaWorld) dispatch(req *pb.SagaHandleRequest) {
	w.resp, w.err = w.router.DispatchSaga(req)
}

// sagaEventSource is a SagaHandleRequest whose source carries one event of fq
// in the "order" domain, at sequence seq when non-nil.
func sagaEventSource(fq string, seq *uint32) *pb.SagaHandleRequest {
	page := &pb.EventPage{Payload: &pb.EventPage_Event{Event: &anypb.Any{TypeUrl: typeURL(fq)}}}
	if seq != nil {
		page.Header = sequenceHeader(*seq)
	}
	return &pb.SagaHandleRequest{
		Source: &pb.EventBook{
			Cover: &pb.Cover{Domain: "order"},
			Pages: []*pb.EventPage{page},
		},
	}
}

// sagaRejectionSource is a SagaHandleRequest whose source is a rejection
// Notification for fqCommand (sagas receive no rejections, so it emits
// nothing).
func sagaRejectionSource(fqCommand string) *pb.SagaHandleRequest {
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
	return &pb.SagaHandleRequest{
		Source: &pb.EventBook{
			Cover: &pb.Cover{Domain: "order"},
			Pages: []*pb.EventPage{{Payload: &pb.EventPage_Event{
				Event: &anypb.Any{
					TypeUrl: typeURL("io.angzarr.v1.Notification"),
					Value:   mustMarshal(notification),
				},
			}}},
		},
	}
}

// --- When ---

func (w *sagaWorld) increasedAt(seq int) {
	s := uint32(seq)
	w.dispatch(sagaEventSource(fqIncreased, &s))
}

func (w *sagaWorld) reserveEvent() {
	w.dispatch(sagaEventSource(fqReserve, nil))
}

func (w *sagaWorld) sourceNoPages() {
	w.dispatch(&pb.SagaHandleRequest{Source: &pb.EventBook{}})
}

func (w *sagaWorld) requestNoSource() {
	w.dispatch(&pb.SagaHandleRequest{})
}

func (w *sagaWorld) rejectionReserve() {
	w.dispatch(sagaRejectionSource(fqReserve))
}

// --- Then ---

func (w *sagaWorld) emitsOneCommand(target string) error {
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

func (w *sagaWorld) commandIsDeferred(seq, index int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	return assertDeferred(w.resp.GetCommands()[0], "order", seq, index)
}

func (w *sagaWorld) emitsNoCommands() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetCommands()); got != 0 {
		return fmt.Errorf("emitted %d commands, want 0", got)
	}
	return nil
}

func (w *sagaWorld) injectsNoEvents() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetEvents()); got != 0 {
		return fmt.Errorf("injected %d events, want 0", got)
	}
	return nil
}

func (w *sagaWorld) failsWith(code string) error {
	var ce *CodedError
	if !errors.As(w.err, &ce) {
		return fmt.Errorf("expected coded error %s, got %v", code, w.err)
	}
	if ce.Code != code {
		return fmt.Errorf("expected %s, got %s", code, ce.Code)
	}
	return nil
}

func initializeSagaScenario(sc *godog.ScenarioContext) {
	w := &sagaWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^an order saga delivering to "([^"]*)"$`, func(string) {})
	sc.Step(`^an Increased event at sequence (\d+) is dispatched$`, w.increasedAt)
	sc.Step(`^a Reserve event is dispatched$`, w.reserveEvent)
	sc.Step(`^a source with no pages is dispatched$`, w.sourceNoPages)
	sc.Step(`^a request with no source is dispatched$`, w.requestNoSource)
	sc.Step(`^a rejection of Reserve is dispatched$`, w.rejectionReserve)
	sc.Step(`^the saga emits one command to "([^"]*)"$`, w.emitsOneCommand)
	sc.Step(`^the command is deferred from source sequence (\d+) at command index (\d+)$`, w.commandIsDeferred)
	sc.Step(`^the saga emits no commands$`, w.emitsNoCommands)
	sc.Step(`^the saga injects no events$`, w.injectsNoEvents)
	sc.Step(`^the dispatch fails with ([A-Z_]+)$`, w.failsWith)
}
