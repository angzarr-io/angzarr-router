//go:build ffirouter

package ffirouter

import (
	"errors"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// Fully-qualified type names the CounterAggregate keys on (FIXTURE.md).
const (
	fqIncreased  = "test.counter.Increased"
	fqIncreaseBy = "test.counter.IncreaseBy"
	fqFailHard   = "test.counter.FailHard"
	fqReserve    = "test.counter.Reserve"
)

// counterState is the host state — it never crosses the FFI.
type counterState struct{ count uint32 }

// counterAggregate is the CounterAggregate fixture (FIXTURE.md) in Go: the
// same behavior the Rust harness implements. observed, when non-nil,
// records the CommandContext and rebuilt count each command handler saw —
// the historical-state evidence the suite asserts, since state never
// crosses the boundary.
func counterAggregate(observed *[]observation) *AggregateDispatch[*counterState] {
	rb := NewRebuilder(func() *counterState { return &counterState{} }).
		Apply(fqIncreased, func(s *counterState, payload *anypb.Any) error {
			var ev counter.Increased
			if err := proto.Unmarshal(payload.Value, &ev); err != nil {
				return err
			}
			s.count++
			return nil
		}).
		WithSnapshot(func(s *counterState, payload *anypb.Any) error {
			var snap counter.CounterState
			if err := proto.Unmarshal(payload.Value, &snap); err != nil {
				return err
			}
			s.count = snap.Count
			return nil
		})

	return NewAggregateDispatch("counter-aggregate", "counter", rb).
		OnCommand(fqIncreaseBy, func(cmd *anypb.Any, s *counterState, cctx CommandContext) (*pb.EventBook, error) {
			if observed != nil {
				*observed = append(*observed, observation{cctx: cctx, count: s.count})
			}
			var c counter.IncreaseBy
			if err := proto.Unmarshal(cmd.Value, &c); err != nil {
				return nil, err
			}
			if c.N == 0 {
				return nil, Reject("VALUE_NOT_POSITIVE", "increase amount must be positive")
			}
			pages := make([]*pb.EventPage, c.N)
			for i := range pages {
				pages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: increasedAny()}}
			}
			return &pb.EventBook{Pages: pages}, nil
		}).
		OnCommand(fqFailHard, func(_ *anypb.Any, _ *counterState, _ CommandContext) (*pb.EventBook, error) {
			return nil, errors.New("hard failure")
		}).
		OnRejected(fqReserve, func(_ *pb.Notification, _ *pb.RejectionNotification, _ *counterState, _ CommandContext) (*pb.BusinessResponse, error) {
			return markerResponse("CompensatedFirst"), nil
		}).
		OnRejected(fqReserve, func(_ *pb.Notification, _ *pb.RejectionNotification, _ *counterState, _ CommandContext) (*pb.BusinessResponse, error) {
			return markerResponse("CompensatedSecond"), nil
		})
}

// observation is one record of what a command handler saw: the context and
// the rebuilt state count.
type observation struct {
	cctx  CommandContext
	count uint32
}

// increasedAny is a single Increased event payload, Any-wrapped with the
// framework's bare-"/" type URL (not the type.googleapis.com Any default).
func increasedAny() *anypb.Any {
	return &anypb.Any{TypeUrl: typeURL(fqIncreased), Value: mustMarshal(&counter.Increased{})}
}

// markerResponse is a compensation response carrying one marker event whose
// type name records which compensator ran (no message body needed — the
// suite asserts on the type URL).
func markerResponse(name string) *pb.BusinessResponse {
	return &pb.BusinessResponse{
		Result: &pb.BusinessResponse_Events{Events: &pb.EventBook{
			Pages: []*pb.EventPage{{Payload: &pb.EventPage_Event{
				Event: &anypb.Any{TypeUrl: typeURL("test.counter." + name)},
			}}},
		}},
	}
}

// Command and prior-history builders live in builders_test.go (skeleton
// parsing, shared with the godog harness).
