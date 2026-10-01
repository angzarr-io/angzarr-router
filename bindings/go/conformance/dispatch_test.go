//go:build ffirouter

package conformance

import (
	"errors"
	"testing"

	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// newCounterRouter registers the CounterAggregate fixture and returns the
// router plus the handler's observation log.
func newCounterRouter(t *testing.T) (*Router, *[]observation) {
	t.Helper()
	r := NewRouter()
	t.Cleanup(r.Close)
	observed := &[]observation{}
	if err := counter.RegisterCounterAggregate(r, counterAggregate{observed}); err != nil {
		t.Fatalf("register: %v", err)
	}
	return r, observed
}

// A command emits one event per unit and stamps consecutive sequences —
// the full round trip across the seam: descriptor, dispatch, command
// callback, EventBook marshaling, and the core's sequence stamping.
func TestDispatch_IncreaseByEmitsSequencedEvents(t *testing.T) {
	r, _ := newCounterRouter(t)
	resp, err := r.Dispatch(increaseCommand(3))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	book := resp.GetEvents()
	if book == nil {
		t.Fatal("expected an events result")
	}
	if len(book.Pages) != 3 {
		t.Fatalf("pages = %d, want 3", len(book.Pages))
	}
	for i, p := range book.Pages {
		if got := p.GetHeader().GetSequence(); got != uint32(i) {
			t.Errorf("page %d sequence = %d, want %d", i, got, i)
		}
	}
}

// A coded business rejection crosses back as a *CodedError carrying the
// stable reason — the google.rpc.Status/ErrorInfo round trip.
func TestDispatch_IncreaseByZeroIsCoded(t *testing.T) {
	r, _ := newCounterRouter(t)
	_, err := r.Dispatch(increaseCommand(0))
	var ce *CodedError
	if !errors.As(err, &ce) {
		t.Fatalf("err = %v (%T), want *CodedError", err, err)
	}
	if ce.Code != "VALUE_NOT_POSITIVE" {
		t.Errorf("code = %q, want VALUE_NOT_POSITIVE", ce.Code)
	}
}

// An unclassified handler error is classified by the binding as
// UNHANDLED_HANDLER_ERROR before it crosses the seam.
func TestDispatch_FailHardIsUnhandled(t *testing.T) {
	r, _ := newCounterRouter(t)
	_, derr := r.Dispatch(failHardCommand())
	var ce *CodedError
	if !errors.As(derr, &ce) || ce.Code != "UNHANDLED_HANDLER_ERROR" {
		t.Fatalf("err = %v, want UNHANDLED_HANDLER_ERROR", derr)
	}
}

// Prior events fold into state and reach the handler as historical
// evidence — the CommandContextAux decode plus applier execution across
// the seam. A fresh aggregate reports no prior history.
func TestDispatch_PriorEventsReachHandlerContext(t *testing.T) {
	r, observed := newCounterRouter(t)

	if _, err := r.Dispatch(increaseCommand(1)); err != nil {
		t.Fatalf("fresh dispatch: %v", err)
	}
	fresh := (*observed)[0]
	if fresh.cctx.HadPriorEvents {
		t.Error("fresh: HadPriorEvents = true, want false")
	}
	if fresh.cctx.NextSequence != 0 || fresh.count != 0 {
		t.Errorf("fresh: ctx=%+v count=%d, want next 0 / count 0", fresh.cctx, fresh.count)
	}

	cmd := increaseCommand(1)
	cmd.Events = priorIncreases(2)
	if _, err := r.Dispatch(cmd); err != nil {
		t.Fatalf("prior dispatch: %v", err)
	}
	prior := (*observed)[1]
	if !prior.cctx.HadPriorEvents {
		t.Error("prior: HadPriorEvents = false, want true")
	}
	if prior.cctx.NextSequence != 2 || prior.count != 2 {
		t.Errorf("prior: ctx=%+v count=%d, want next 2 / count 2", prior.cctx, prior.count)
	}
}

// Two compensators registered for one rejected command on one aggregate run
// across the FFI in registration order, and the core merges their events in
// that order: the first page comes from the first compensator.
func TestDispatch_TwoCompensatorsFanOutInRegistrationOrder(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)

	var ran []string
	compensator := func(name string) RejectionThunk[*counter.CounterState] {
		return func(*pb.Notification, *pb.RejectionNotification, *counter.CounterState, CommandContext) (*pb.BusinessResponse, error) {
			ran = append(ran, name)
			return &pb.BusinessResponse{Result: &pb.BusinessResponse_Events{Events: &pb.EventBook{
				Pages: []*pb.EventPage{markerPage(name)},
			}}}, nil
		}
	}
	rebuilder := NewRebuilder(func() *counter.CounterState { return &counter.CounterState{} })
	d := NewAggregateDispatch("TwoCompensators", "counter", rebuilder).
		OnRejected(fqReserve, compensator("CompensatedFirst")).
		OnRejected(fqReserve, compensator("CompensatedSecond"))
	if err := RegisterAggregate(r, d); err != nil {
		t.Fatalf("register: %v", err)
	}

	resp, err := r.Dispatch(rejectionCommand(fqReserve))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	want := []string{"CompensatedFirst", "CompensatedSecond"}
	if len(ran) != len(want) || ran[0] != want[0] || ran[1] != want[1] {
		t.Fatalf("compensators ran %v, want %v", ran, want)
	}
	pages := resp.GetEvents().GetPages()
	if len(pages) != len(want) {
		t.Fatalf("merged pages = %d, want %d", len(pages), len(want))
	}
	for i, p := range pages {
		if got := fqFromURL(p.GetEvent().GetTypeUrl()); got != "test.counter."+want[i] {
			t.Errorf("page %d = %s, want test.counter.%s", i, got, want[i])
		}
	}
}

// A saga's Destinations are exactly its registered target domains, its
// handler receives the source cover, and the command it returns comes back
// stamped deferred by the router.
func TestDispatchSaga_DestinationsAreTheTargetDomains(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)

	var seen []string
	var sourceDomain string
	d := NewSagaDispatch("Shipper", "order", "inventory", "billing").
		OnEvent(fqIncreased, func(_ *anypb.Any, dests *Destinations, sourceCover *pb.Cover) ([]*pb.CommandBook, []*pb.EventBook, error) {
			seen = dests.Domains()
			sourceDomain = sourceCover.GetDomain()
			return []*pb.CommandBook{reserveCommand()}, nil, nil
		})
	if err := r.RegisterSaga(d); err != nil {
		t.Fatalf("register: %v", err)
	}
	seq := uint32(3)
	resp, err := r.DispatchSaga(sagaEventSource(fqIncreased, &seq))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	if len(seen) != 2 || seen[0] != "inventory" || seen[1] != "billing" {
		t.Errorf("saga saw destinations %v, want [inventory billing]", seen)
	}
	if sourceDomain != "order" {
		t.Errorf("saga saw source cover domain %q, want \"order\"", sourceDomain)
	}
	if err := assertDeferred(resp.GetCommands()[0], "order", 3, 0); err != nil {
		t.Error(err)
	}
}

// A process manager's Destinations are the target domains given at
// construction.
func TestDispatchProcessManager_DestinationsAreTheTargetDomains(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)

	var seen []string
	rebuilder := NewRebuilder(func() *counter.OrderProcessManagerState { return &counter.OrderProcessManagerState{} })
	d := NewProcessManagerDispatch("Targeted", "targeted-pm", rebuilder, "inventory").
		OnEvent("counter", fqIncreased, func(_ *anypb.Any, _ *counter.OrderProcessManagerState, dests *Destinations) (*pb.ProcessManagerHandleResponse, error) {
			seen = dests.Domains()
			return &pb.ProcessManagerHandleResponse{Commands: []*pb.CommandBook{reserveCommand()}}, nil
		})
	if err := RegisterProcessManager(r, d); err != nil {
		t.Fatalf("register: %v", err)
	}
	seq := uint32(9)
	resp, err := r.DispatchProcessManager(pmTrigger("counter", []string{fqIncreased}, nil, &seq))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	if len(seen) != 1 || seen[0] != "inventory" {
		t.Errorf("PM saw destinations %v, want [inventory]", seen)
	}
	if err := assertDeferred(resp.GetCommands()[0], "counter", 9, 0); err != nil {
		t.Error(err)
	}
}

// An undo handler receives the Compensate the notification carries and the
// cover of the delivery it handles.
func TestDispatch_UndoHandlerSeesCompensateAndCover(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)

	var gotType string
	var gotCover *pb.Cover
	d := NewAggregateDispatch("Inventory", "inventory", NewRebuilder(newNoState)).
		OnUndo(fqReserve, func(_ *pb.Notification, c *pb.Compensate, _ noState, cctx CommandContext) (*pb.BusinessResponse, error) {
			gotType, gotCover = c.GetCommandType(), cctx.Cover
			return nil, nil
		})
	if err := RegisterAggregate(r, d); err != nil {
		t.Fatalf("register: %v", err)
	}
	resp, err := r.Dispatch(notificationCommand("inventory", compensatePayload(fqReserve), nil))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	if gotType != fqReserve {
		t.Errorf("undo saw command_type %q, want %q", gotType, fqReserve)
	}
	if gotCover.GetDomain() != "inventory" {
		t.Errorf("undo saw cover %v, want domain inventory", gotCover)
	}
	if n := len(resp.GetEvents().GetPages()); n != 0 {
		t.Errorf("a nil undo response emitted %d events, want 0", n)
	}
}

// An aggregate whose state is not a protobuf message registers no state
// packer, so Replay is refused as NO_HANDLER_REGISTERED.
func TestDispatchReplay_NonMessageStateIsUnsupported(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)
	if err := RegisterAggregate(r, NewAggregateDispatch("Inventory", "inventory", NewRebuilder(newNoState))); err != nil {
		t.Fatalf("register: %v", err)
	}
	_, err := r.DispatchReplay("inventory", &pb.ReplayRequest{})
	var ce *CodedError
	if !errors.As(err, &ce) || ce.Code != "NO_HANDLER_REGISTERED" {
		t.Fatalf("err = %v, want NO_HANDLER_REGISTERED", err)
	}
}

// OnRejected and OnRejectedResponse share one ordered fan-out for an entry:
// process events merge in registration order and the full-response
// compensator's commands come back stamped deferred.
func TestDispatchProcessManager_CompensatorVariantsFanOutInOrder(t *testing.T) {
	r := NewRouter()
	t.Cleanup(r.Close)

	rebuilder := NewRebuilder(func() *counter.OrderProcessManagerState { return &counter.OrderProcessManagerState{} })
	d := NewProcessManagerDispatch("Mixed", "mixed-pm", rebuilder, "inventory").
		OnRejected(fqReserve, func(*pb.Notification, *pb.RejectionNotification, *counter.OrderProcessManagerState) ([]*pb.EventBook, *pb.Notification, error) {
			return []*pb.EventBook{{Cover: &pb.Cover{Domain: "first"}}}, nil, nil
		}).
		OnRejectedResponse(fqReserve, func(*pb.Notification, *pb.RejectionNotification, *counter.OrderProcessManagerState) (*pb.ProcessManagerHandleResponse, error) {
			return &pb.ProcessManagerHandleResponse{
				ProcessEvents: []*pb.EventBook{{Cover: &pb.Cover{Domain: "second"}}},
				Commands:      []*pb.CommandBook{reserveCommand()},
			}, nil
		})
	if err := RegisterProcessManager(r, d); err != nil {
		t.Fatalf("register: %v", err)
	}
	req := pmRejection(fqReserve)
	req.Trigger.Pages[0].Header = sequenceHeader(5)
	resp, err := r.DispatchProcessManager(req)
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	events := resp.GetProcessEvents()
	if len(events) != 2 || events[0].GetCover().GetDomain() != "first" || events[1].GetCover().GetDomain() != "second" {
		t.Fatalf("process events %v, want [first second]", events)
	}
	if len(resp.GetCommands()) != 1 {
		t.Fatalf("commands = %d, want 1", len(resp.GetCommands()))
	}
	if d := resp.GetCommands()[0].GetPages()[0].GetHeader().GetAngzarrDeferred(); d == nil || d.GetSourceSeq() != 5 {
		t.Errorf("compensator command header %v, want deferred from source sequence 5", resp.GetCommands()[0].GetPages()[0].GetHeader())
	}
}
