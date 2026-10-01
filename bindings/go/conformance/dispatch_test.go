//go:build ffirouter

package conformance

import (
	"errors"
	"testing"

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
