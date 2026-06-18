//go:build ffirouter

package ffirouter

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// TestSagaConformance runs the shared saga.feature behavior suite against the
// Go binding via godog — the same feature the Rust cucumber-rs harness drives
// against the core. Only the step layer is new.
func TestSagaConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeSagaScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../conformance/features/saga.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("saga conformance scenarios failed")
	}
}

// orderSaga is the OrderSaga fixture in Go (the saga.feature behavior): it
// translates each Increased source event into one Reserve command for
// "inventory" (stamped from the destination sequence when present), and
// compensates a rejected Reserve by injecting one fact event.
func orderSaga() *SagaDispatch {
	return NewSagaDispatch("order-saga", "order", "inventory").
		OnEvent(fqIncreased, func(_ *anypb.Any, dests *Destinations) ([]*pb.CommandBook, []*pb.EventBook, error) {
			cmd := &pb.CommandBook{
				Cover: &pb.Cover{Domain: "inventory"},
				Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
					Command: &anypb.Any{TypeUrl: typeURL(fqReserve)},
				}}},
			}
			if dests.Has("inventory") {
				if err := dests.StampCommand(cmd, "inventory"); err != nil {
					return nil, nil, err
				}
			}
			return []*pb.CommandBook{cmd}, nil, nil
		}).
		OnRejected(fqReserve, func(_ *pb.Notification, _ *pb.RejectionNotification) ([]*pb.EventBook, error) {
			return []*pb.EventBook{{Pages: []*pb.EventPage{{}}}}, nil
		})
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
	if err := w.router.RegisterSaga(orderSaga()); err != nil {
		panic(fmt.Sprintf("register saga fixture: %v", err))
	}
}

func (w *sagaWorld) dispatch(req *pb.SagaHandleRequest) {
	w.resp, w.err = w.router.DispatchSaga(req)
}

// sagaEventSource is a SagaHandleRequest whose source carries one event of fq
// in the "order" domain, plus the coordinator's destination-sequence map.
func sagaEventSource(fq string, dest map[string]uint32) *pb.SagaHandleRequest {
	return &pb.SagaHandleRequest{
		Source: &pb.EventBook{
			Cover: &pb.Cover{Domain: "order"},
			Pages: []*pb.EventPage{{Payload: &pb.EventPage_Event{
				Event: &anypb.Any{TypeUrl: typeURL(fq)},
			}}},
		},
		DestinationSequences: dest,
	}
}

// sagaRejectionSource is a SagaHandleRequest whose source is a rejection
// Notification for fqCommand — routes to the compensation path.
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

func (w *sagaWorld) increasedWithDestination(seq int) {
	w.dispatch(sagaEventSource(fqIncreased, map[string]uint32{"inventory": uint32(seq)}))
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

func (w *sagaWorld) rejectionUnwatched() {
	w.dispatch(sagaRejectionSource("test.counter.Unwatched"))
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

func (w *sagaWorld) commandCarriesSequence(seq int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	got := w.resp.GetCommands()[0].GetPages()[0].GetHeader().GetSequence()
	if got != uint32(seq) {
		return fmt.Errorf("command carries sequence %d, want %d", got, seq)
	}
	return nil
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

func (w *sagaWorld) injectsOneEvent() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetEvents()); got != 1 {
		return fmt.Errorf("injected %d events, want 1", got)
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
	sc.Step(`^an Increased event is dispatched with destination inventory sequence (\d+)$`, w.increasedWithDestination)
	sc.Step(`^a Reserve event is dispatched$`, w.reserveEvent)
	sc.Step(`^a source with no pages is dispatched$`, w.sourceNoPages)
	sc.Step(`^a request with no source is dispatched$`, w.requestNoSource)
	sc.Step(`^a rejection of Reserve is dispatched$`, w.rejectionReserve)
	sc.Step(`^a rejection of Unwatched is dispatched$`, w.rejectionUnwatched)
	sc.Step(`^the saga emits one command to "([^"]*)"$`, w.emitsOneCommand)
	sc.Step(`^the command carries destination sequence (\d+)$`, w.commandCarriesSequence)
	sc.Step(`^the saga emits no commands$`, w.emitsNoCommands)
	sc.Step(`^the saga injects one fact event$`, w.injectsOneEvent)
	sc.Step(`^the saga injects no events$`, w.injectsNoEvents)
	sc.Step(`^the dispatch fails with ([A-Z_]+)$`, w.failsWith)
}
