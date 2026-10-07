package ffirouter

import (
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// PMEventThunk handles the newest trigger event against rebuilt PM state,
// returning the full response (process events, commands, facts, optional
// escalation). dests are the PM's declared target domains; commands are
// returned unstamped and the router stamps them deferred from the trigger. Binding/generated thunks unmarshal to the typed event and call
// the typed business method.
type PMEventThunk[S any] func(event *anypb.Any, state S, dests *Destinations) (*pb.ProcessManagerHandleResponse, error)

// PMEventCoverThunk is a PMEventThunk that also receives the trigger book's
// cover, so the handler can read the trigger's identity (root, ext).
type PMEventCoverThunk[S any] func(event *anypb.Any, state S, dests *Destinations, triggerCover *pb.Cover) (*pb.ProcessManagerHandleResponse, error)

// PMCompensatorThunk compensates a rejected PM-issued command against rebuilt
// state, returning the full response: process events, commands (stamped
// deferred by the router), facts and an optional escalation. Multiple
// compensators for one entry run in registration order (C-0042); their
// responses merge and the first escalation wins.
type PMCompensatorThunk[S any] func(n *pb.Notification, rejection *pb.RejectionNotification, state S) (*pb.ProcessManagerHandleResponse, error)

// PMRejectionThunk compensates a rejected PM-issued command against rebuilt
// state, returning process events and an optional escalation Notification.
// Multiple thunks for one command run in registration order (C-0042); their
// process events merge and the first escalation wins.
type PMRejectionThunk[S any] func(n *pb.Notification, rejection *pb.RejectionNotification, state S) ([]*pb.EventBook, *pb.Notification, error)

// ProcessManagerDispatch is one process-manager component's registration: its
// name, its own domain, its declared target (output) domains, the rebuilder
// for its event-sourced state, event handlers keyed by (input domain, FQ event
// type), and ordered rejection compensators. The shape mirrors the core's so
// generated wiring targets it with minimal emitter changes.
type ProcessManagerDispatch[S any] struct {
	name       string
	pmDomain   string
	targets    []string
	rebuilder  *Rebuilder[S]
	handlers   map[string]map[string]PMEventCoverThunk[S]
	rejections map[string][]PMCompensatorThunk[S]
}

// NewProcessManagerDispatch starts a PM registration over a Rebuilder for the
// PM's own event-sourced state. targetDomains are the PM's declared output
// domains (the Destinations its handlers see).
func NewProcessManagerDispatch[S any](name, pmDomain string, rebuilder *Rebuilder[S], targetDomains ...string) *ProcessManagerDispatch[S] {
	return &ProcessManagerDispatch[S]{
		name:       name,
		pmDomain:   pmDomain,
		targets:    targetDomains,
		rebuilder:  rebuilder,
		handlers:   make(map[string]map[string]PMEventCoverThunk[S]),
		rejections: make(map[string][]PMCompensatorThunk[S]),
	}
}

// OnEvent registers the thunk for (input domain, fully-qualified event type).
func (d *ProcessManagerDispatch[S]) OnEvent(inputDomain, fullName string, thunk PMEventThunk[S]) *ProcessManagerDispatch[S] {
	return d.OnEventWithCover(inputDomain, fullName, func(event *anypb.Any, state S, dests *Destinations, _ *pb.Cover) (*pb.ProcessManagerHandleResponse, error) {
		return thunk(event, state, dests)
	})
}

// OnEventWithCover registers the thunk for (input domain, fully-qualified
// event type); the thunk also receives the trigger book's cover.
func (d *ProcessManagerDispatch[S]) OnEventWithCover(inputDomain, fullName string, thunk PMEventCoverThunk[S]) *ProcessManagerDispatch[S] {
	if d.handlers[inputDomain] == nil {
		d.handlers[inputDomain] = make(map[string]PMEventCoverThunk[S])
	}
	d.handlers[inputDomain][fullName] = thunk
	return d
}

// OnRejected appends a compensator for one compensates entry — the rejected
// command's fully-qualified type ("fq.Type", sent to any domain) or
// "domain:fq.Type" (only when it was sent to that domain); repeated calls
// register an ordered fan-out (C-0042).
func (d *ProcessManagerDispatch[S]) OnRejected(fqCommand string, thunk PMRejectionThunk[S]) *ProcessManagerDispatch[S] {
	return d.OnRejectedResponse(fqCommand, func(n *pb.Notification, rejection *pb.RejectionNotification, state S) (*pb.ProcessManagerHandleResponse, error) {
		processEvents, escalation, err := thunk(n, rejection, state)
		if err != nil {
			return nil, err
		}
		return &pb.ProcessManagerHandleResponse{ProcessEvents: processEvents, Notification: escalation}, nil
	})
}

// OnRejectedResponse appends a compensator returning the full
// ProcessManagerHandleResponse for one compensates entry ("fq.Type" or
// "domain:fq.Type"); it shares the ordered fan-out with OnRejected.
func (d *ProcessManagerDispatch[S]) OnRejectedResponse(compensates string, thunk PMCompensatorThunk[S]) *ProcessManagerDispatch[S] {
	d.rejections[compensates] = append(d.rejections[compensates], thunk)
	return d
}
