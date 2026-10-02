//go:build ffirouter

package conformance

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	abipb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/router/ffi/v1"
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
	// seen records the source sequence the Increased handler was given.
	seen []uint32
	// registration is the outcome of a raw saga descriptor registration.
	registration error
	registered   bool
}

func (w *sagaWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.resp = nil
	w.err = nil
	w.seen = nil
	w.registration = nil
	w.registered = false
	saga := counter.NewOrderSagaDispatch(orderSaga{}).OnEventWithContext(fqIncreased,
		func(event *anypb.Any, dests *Destinations, source PageContext) ([]*pb.CommandBook, []*pb.EventBook, error) {
			w.seen = append(w.seen, source.Sequence)
			var increased counter.Increased
			if err := event.UnmarshalTo(&increased); err != nil {
				return nil, nil, err
			}
			return orderSaga{}.Increased(&increased, dests, source)
		})
	if err := w.router.RegisterSaga(saga); err != nil {
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

// rangeBytes is the bytes lo, lo+1, ..., hi.
func rangeBytes(lo, hi byte) []byte {
	b := make([]byte, 0, int(hi-lo)+1)
	for v := lo; ; v++ {
		b = append(b, v)
		if v == hi {
			return b
		}
	}
}

// parityCommand is the parity command: cover "inventory", root bytes
// 10..1f, correlation "corr-1"; one page whose command is "/example.Foo"
// carrying 01020304.
func parityCommand() *pb.CommandBook {
	return &pb.CommandBook{
		Cover: &pb.Cover{
			Domain:        "inventory",
			Root:          &pb.UUID{Value: rangeBytes(0x10, 0x1f)},
			CorrelationId: "corr-1",
		},
		Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
			Command: &anypb.Any{TypeUrl: "/example.Foo", Value: []byte{1, 2, 3, 4}},
		}}},
	}
}

// paritySaga is the parity saga ("order" -> "inventory"): its Increased
// handler emits parityCommand twice.
func paritySaga() *SagaDispatch {
	return NewSagaDispatch("parity-saga", "order", "inventory").OnEvent(fqIncreased,
		func(*anypb.Any, *Destinations, *pb.Cover) ([]*pb.CommandBook, []*pb.EventBook, error) {
			return []*pb.CommandBook{parityCommand(), parityCommand()}, nil, nil
		})
}

// paritySource is one Increased event at seq under cover "order", root bytes
// 00..0f, correlation "corr-1".
func paritySource(seq uint32) *pb.SagaHandleRequest {
	req := sagaEventSource(fqIncreased, &seq)
	req.Source.Cover = &pb.Cover{
		Domain:        "order",
		Root:          &pb.UUID{Value: rangeBytes(0x00, 0x0f)},
		CorrelationId: "corr-1",
	}
	return req
}

// --- Given ---

func (w *sagaWorld) aParitySaga() error {
	w.router.Close()
	w.router = NewRouter()
	return w.router.RegisterSaga(paritySaga())
}

// --- When ---

func (w *sagaWorld) rootedIncreasedAt(label string, seq int) {
	s := uint32(seq)
	req := sagaEventSource(fqIncreased, &s)
	req.Source.Cover = coverOf("order", label)
	w.dispatch(req)
}

func (w *sagaWorld) paritySourceAt(seq int) {
	w.dispatch(paritySource(uint32(seq)))
}

// registerCompensatingSaga registers, through the binding's low-level entry
// point, a saga descriptor declaring a compensation for Reserve: the typed
// SagaDispatch has no way to declare one.
func (w *sagaWorld) registerCompensatingSaga() {
	r := NewRouter()
	defer r.Close()
	w.registration = r.RegisterSagaDescriptor(&abipb.SagaDescriptor{
		Name:          "order-saga",
		InputDomain:   "order",
		TargetDomains: []string{"inventory"},
		Rejections: []*abipb.RejectionEntry{{
			Compensates: fqReserve,
			CallbackIds: []uint64{1},
		}},
	})
	w.registered = true
}

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

func (w *sagaWorld) leavesSourceComponent() error {
	if err := w.emitsOneCommand("inventory"); err != nil {
		return err
	}
	return assertNoSourceComponent(w.resp.GetCommands()[0])
}

func (w *sagaWorld) deferredFromRoot(label string) error {
	if err := w.emitsOneCommand("inventory"); err != nil {
		return err
	}
	want := coverOf("order", label)
	for i, page := range w.resp.GetCommands()[0].GetPages() {
		d := page.GetHeader().GetAngzarrDeferred()
		if d == nil {
			return fmt.Errorf("command page %d is not deferred", i)
		}
		if !proto.Equal(d.GetSource(), want) {
			return fmt.Errorf("page %d deferred source = %v, want the whole cover %v", i, d.GetSource(), want)
		}
	}
	return nil
}

func (w *sagaWorld) commandHashes(index int, want string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	commands := w.resp.GetCommands()
	if index >= len(commands) {
		return fmt.Errorf("emitted %d commands, no index %d", len(commands), index)
	}
	b, err := proto.MarshalOptions{Deterministic: true}.Marshal(commands[index])
	if err != nil {
		return fmt.Errorf("marshal command %d: %w", index, err)
	}
	sum := sha256.Sum256(b)
	if got := hex.EncodeToString(sum[:]); got != want {
		return fmt.Errorf("command %d hashes to %s, want %s", index, got, want)
	}
	return nil
}

func (w *sagaWorld) registrationRefused() error {
	if !w.registered {
		return errors.New("no saga registration was attempted")
	}
	var ce *CodedError
	if !errors.As(w.registration, &ce) {
		return fmt.Errorf("expected a coded refusal, got %v", w.registration)
	}
	if ce.Grpc != GrpcInvalidArgument {
		return fmt.Errorf("refused as gRPC %d, want INVALID_ARGUMENT (3)", ce.Grpc)
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

func (w *sagaWorld) injectsNoEvents() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if got := len(w.resp.GetEvents()); got != 0 {
		return fmt.Errorf("injected %d events, want 0", got)
	}
	return nil
}

func (w *sagaWorld) handlerSawSequence(seq int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if len(w.seen) != 1 || w.seen[0] != uint32(seq) {
		return fmt.Errorf("saga handler saw source sequences %v, want [%d]", w.seen, seq)
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
	sc.Step(`^a parity saga emitting the parity command twice$`, w.aParitySaga)
	sc.Step(`^an Increased event at sequence (\d+) is dispatched$`, w.increasedAt)
	sc.Step(`^an Increased event of order root "([^"]*)" at sequence (\d+) is dispatched$`, w.rootedIncreasedAt)
	sc.Step(`^the parity source event at sequence (\d+) is dispatched$`, w.paritySourceAt)
	sc.Step(`^a saga declaring a compensation for Reserve is registered$`, w.registerCompensatingSaga)
	sc.Step(`^the command leaves its source component to the coordinator$`, w.leavesSourceComponent)
	sc.Step(`^the command is deferred from order root "([^"]*)"$`, w.deferredFromRoot)
	sc.Step(`^the command at index (\d+) hashes to SHA-256 "([0-9a-f]{64})"$`, w.commandHashes)
	sc.Step(`^the registration is refused as INVALID_ARGUMENT$`, w.registrationRefused)
	sc.Step(`^a Reserve event is dispatched$`, w.reserveEvent)
	sc.Step(`^a source with no pages is dispatched$`, w.sourceNoPages)
	sc.Step(`^a request with no source is dispatched$`, w.requestNoSource)
	sc.Step(`^a rejection of Reserve is dispatched$`, w.rejectionReserve)
	sc.Step(`^the saga emits one command to "([^"]*)"$`, w.emitsOneCommand)
	sc.Step(`^the command is deferred from source sequence (\d+) at command index (\d+)$`, w.commandIsDeferred)
	sc.Step(`^the saga emits no commands$`, w.emitsNoCommands)
	sc.Step(`^the saga injects no events$`, w.injectsNoEvents)
	sc.Step(`^the saga handler saw source sequence (\d+)$`, w.handlerSawSequence)
	sc.Step(`^the dispatch fails with ([A-Z_]+)$`, w.failsWith)
}
