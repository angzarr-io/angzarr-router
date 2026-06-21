//go:build ffirouter

package conformance

// The conformance fixtures, implementing the angzarr-generated Handler
// interfaces (gen/test/counter/*_angzarr.pb.go). The behaviour is the
// same the hand-written dispatches encoded; the wiring is now generated, so
// these are the proof the generated seam is faithful. Registered via the
// generated Register<Component> helpers in each scenario's world.

import (
	"errors"

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

func (counterAggregate) ApplyIncreased(state *counter.CounterState, _ *counter.Increased) {
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

func (orderSaga) Increased(_ *counter.Increased, dests *Destinations) ([]*pb.CommandBook, []*pb.EventBook, error) {
	cmd := reserveCommand()
	if dests.Has("inventory") {
		if err := dests.StampCommand(cmd, "inventory"); err != nil {
			return nil, nil, err
		}
	}
	return []*pb.CommandBook{cmd}, nil, nil
}

func (orderSaga) OnReserveRejected(*pb.Notification, *pb.RejectionNotification) ([]*pb.EventBook, error) {
	return []*pb.EventBook{oneFact()}, nil
}

// --- CounterProjector ---

type counterProjector struct{}

func (counterProjector) Increased(p *counter.CounterProjectorState, _ *counter.Increased) error {
	p.Count++
	return nil
}

func (counterProjector) Finish(p *counter.CounterProjectorState, events *pb.EventBook) (*pb.Projection, error) {
	return &pb.Projection{Cover: events.GetCover(), Projector: "counter-projector", Sequence: p.Count}, nil
}

// --- OrderProcessManager ---

type orderPM struct{}

func (orderPM) Increased(_ *counter.Increased, state *counter.OrderProcessManagerState, dests *Destinations) (*pb.ProcessManagerHandleResponse, error) {
	cmd := reserveCommand()
	if dests.Has("inventory") {
		if err := dests.StampCommand(cmd, "inventory"); err != nil {
			return nil, err
		}
	}
	facts := make([]*pb.EventBook, int(state.Count))
	for i := range facts {
		facts[i] = oneFact()
	}
	return &pb.ProcessManagerHandleResponse{Commands: []*pb.CommandBook{cmd}, Facts: facts}, nil
}

func (orderPM) ApplyIncreased(state *counter.OrderProcessManagerState, _ *counter.Increased) {
	state.Count++
}

func (orderPM) OnReserveRejected(*pb.Notification, *pb.RejectionNotification, *counter.OrderProcessManagerState) ([]*pb.EventBook, *pb.Notification, error) {
	return []*pb.EventBook{oneFact()}, &pb.Notification{Cover: &pb.Cover{Domain: "escalated"}}, nil
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
