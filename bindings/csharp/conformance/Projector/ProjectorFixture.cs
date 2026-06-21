using Angzarr;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Projector;

/// <summary>The conformance CounterProjector fixture: every delivered event
/// folds into one projection; the finisher carries the cover and folded
/// count.</summary>
internal sealed class ProjectorFixture : TC.counter_angzarr.CounterProjectorHandler
{
    public void Increased(TC.CounterProjectorState projection, TC.Increased ev) =>
        projection.Count += 1;

    public Projection Finish(TC.CounterProjectorState projection, EventBook events) =>
        new()
        {
            Cover = events.Cover,
            Projector = "counter-projector",
            Sequence = projection.Count,
        };
}
