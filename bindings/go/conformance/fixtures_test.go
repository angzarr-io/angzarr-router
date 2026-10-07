//go:build ffirouter

package conformance

// The conformance fixtures, implementing the angzarr-generated Handler
// interfaces (gen/test/counter/*_angzarr.pb.go). The behaviour is the
// same the hand-written dispatches encoded; the wiring is now generated, so
// these are the proof the generated seam is faithful. Registered via the
// generated Register<Component> helpers in each scenario's world.

import (
	"errors"
	"fmt"

	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// Fully-qualified type names the fixtures key on (FIXTURE.md).
const (
	fqIncreased  = "test.counter.Increased"
	fqIncreaseBy = "test.counter.IncreaseBy"
	fqFailHard   = "test.counter.FailHard"
	fqReserve    = "test.counter.Reserve"
)

// observation records the CommandContext and rebuilt count a command handler
// saw — the historical-state evidence the suite asserts, since state never
// crosses the boundary.
type observation struct {
	cctx  CommandContext
	count uint32
}

// increasedAny is one Increased event payload, Any-wrapped with the framework's
// bare-"/" type URL.
func increasedAny() *anypb.Any {
	return &anypb.Any{TypeUrl: typeURL(fqIncreased), Value: mustMarshal(&counter.Increased{})}
}

// noState is the stateless host state of hand-built components that
// keep none.
type noState struct{}

func newNoState() noState { return noState{} }

// --- CounterAggregate ---

type counterAggregate struct{ observed *[]observation }

func (f counterAggregate) IncreaseBy(cmd *counter.IncreaseBy, state *counter.CounterState, cctx CommandContext) ([]*counter.Increased, error) {
	if f.observed != nil {
		*f.observed = append(*f.observed, observation{cctx: cctx, count: state.Count})
	}
	if cmd.N == 0 {
		return nil, Reject("VALUE_NOT_POSITIVE", "increase amount must be positive")
	}
	events := make([]*counter.Increased, cmd.N)
	for i := range events {
		events[i] = &counter.Increased{}
	}
	return events, nil
}

func (counterAggregate) FailHard(*counter.FailHard, *counter.CounterState, CommandContext) (*pb.EventBook, error) {
	return nil, errors.New("hard failure")
}

func (counterAggregate) ApplyIncreased(state *counter.CounterState, _ *counter.Increased, _ PageContext) {
	state.Count++
}

// OnReserveRejected appends both ordered markers in one response — the
// within-component fan-out collapses to one compensator (subscriber =
// component), preserving the observable two-marker ordering the feature asserts.
func (counterAggregate) OnReserveRejected(*pb.Notification, *pb.RejectionNotification, *counter.CounterState, CommandContext) (*pb.BusinessResponse, error) {
	return &pb.BusinessResponse{Result: &pb.BusinessResponse_Events{Events: &pb.EventBook{Pages: []*pb.EventPage{
		markerPage("CompensatedFirst"),
		markerPage("CompensatedSecond"),
	}}}}, nil
}

func markerPage(name string) *pb.EventPage {
	return &pb.EventPage{Payload: &pb.EventPage_Event{Event: &anypb.Any{TypeUrl: typeURL("test.counter." + name)}}}
}

// --- OrderSaga ---

type orderSaga struct{}

// Increased emits one Reserve command for "inventory"; the router stamps it
// deferred from the triggering event.
func (orderSaga) Increased(*counter.Increased, *Destinations, PageContext) ([]*pb.CommandBook, []*pb.EventBook, error) {
	return []*pb.CommandBook{reserveCommand()}, nil, nil
}

// --- CounterProjector ---

type counterProjector struct{}

func (counterProjector) Increased(p *counter.CounterProjectorState, _ *counter.Increased, _ PageContext) error {
	p.Count++
	return nil
}

func (counterProjector) Finish(p *counter.CounterProjectorState, events *pb.EventBook) (*pb.Projection, error) {
	return &pb.Projection{Cover: events.GetCover(), Projector: "counter-projector", Sequence: p.Count}, nil
}

// --- OrderProcessManager ---

type orderPM struct {
	seen *rejectionSink
}

// Increased emits one Reserve command for "inventory" (stamped deferred by the
// router) plus one fact per prior state event.
func (orderPM) Increased(_ *counter.Increased, state *counter.OrderProcessManagerState, _ *Destinations, _ *pb.Cover) (*pb.ProcessManagerHandleResponse, error) {
	cmd := reserveCommand()
	facts := make([]*pb.EventBook, int(state.Count))
	for i := range facts {
		facts[i] = oneFact()
	}
	return &pb.ProcessManagerHandleResponse{Commands: []*pb.CommandBook{cmd}, Facts: facts}, nil
}

func (orderPM) ApplyIncreased(state *counter.OrderProcessManagerState, _ *counter.Increased, _ PageContext) {
	state.Count++
}

// OnReserveRejected records the rejection's code and message, then
// compensates with one fact and an escalation.
func (p orderPM) OnReserveRejected(_ *pb.Notification, r *pb.RejectionNotification, _ *counter.OrderProcessManagerState) (*pb.ProcessManagerHandleResponse, error) {
	p.seen.record(r)
	return &pb.ProcessManagerHandleResponse{
		ProcessEvents: []*pb.EventBook{oneFact()},
		Notification:  &pb.Notification{Cover: &pb.Cover{Domain: "escalated"}},
	}, nil
}

// rejectionSink holds the (code, rejection_reason) of each rejection a
// compensator handled.
type rejectionSink [][2]string

func (s *rejectionSink) record(r *pb.RejectionNotification) {
	*s = append(*s, [2]string{r.GetCode(), r.GetRejectionReason()})
}

// exactly reports whether exactly one rejection was handled, with code and
// message.
func (s *rejectionSink) exactly(code, message string) error {
	want := [][2]string{{code, message}}
	if fmt.Sprintf("%q", *s) != fmt.Sprintf("%q", want) {
		return fmt.Errorf("compensators saw (code, message) %q, want %q", *s, want)
	}
	return nil
}

// --- AuditProcessManager ---

// auditMark is the cover domain the audit PM stamps on its facts and process
// events, so scenarios can tell its reactions from the order PM's.
const auditMark = "audit"

// auditPM is co-resident with the order PM over the same trigger and rejected
// command but folds into its own state type: one "audit" fact per prior state
// event and no commands; it compensates with one "audit" process event and no
// escalation.
type auditPM struct{}

func (auditPM) Increased(_ *counter.Increased, state *counter.AuditProcessManagerState, _ *Destinations, _ *pb.Cover) (*pb.ProcessManagerHandleResponse, error) {
	facts := make([]*pb.EventBook, len(state.Seen))
	for i := range facts {
		facts[i] = auditBook()
	}
	return &pb.ProcessManagerHandleResponse{Facts: facts}, nil
}

func (auditPM) ApplyIncreased(state *counter.AuditProcessManagerState, _ *counter.Increased, _ PageContext) {
	state.Seen = append(state.Seen, "Increased")
}

func (auditPM) OnReserveRejected(*pb.Notification, *pb.RejectionNotification, *counter.AuditProcessManagerState) (*pb.ProcessManagerHandleResponse, error) {
	return &pb.ProcessManagerHandleResponse{ProcessEvents: []*pb.EventBook{auditBook()}}, nil
}

func auditBook() *pb.EventBook {
	return &pb.EventBook{Cover: &pb.Cover{Domain: auditMark}}
}

// reserveCommand builds the one-page Reserve command the saga and PM emit for
// the "inventory" domain.
func reserveCommand() *pb.CommandBook {
	return &pb.CommandBook{
		Cover: &pb.Cover{Domain: "inventory"},
		Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{Command: &anypb.Any{TypeUrl: typeURL(fqReserve)}}}},
	}
}

// oneFact is a single empty fact-event book the compensators inject.
func oneFact() *pb.EventBook {
	return &pb.EventBook{Pages: []*pb.EventPage{{}}}
}
