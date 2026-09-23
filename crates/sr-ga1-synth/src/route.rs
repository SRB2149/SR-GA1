//! Negotiated congestion routing (PathFinder) over the routing resource
//! graph.
//!
//! Each lane segment holds one net, which is a hardware fact rather than a
//! modelling choice: every segment is driven by exactly one mux. Congestion
//! is therefore always a real conflict, and PathFinder's job is to decide who
//! yields.
//!
//! Four things here are specific to this fabric.
//!
//! * **Constants are nets, not annotations.** Every input tied to 0 belongs
//!   to one shared constant net, so two inputs of the same cell tying to the
//!   same value share one lane for free — which is exactly the hardware
//!   truth, since input `b` and input `c` overlap on a major lane. The rows
//!   whose IO map supplies a constant at the left edge start with those
//!   segments already in the net, which is why row 3 gets its enables free.
//! * **A buffer's constant is discovered while routing and satisfied in the
//!   same pass.** Taking the operation core as a pass-through turns a spare
//!   CLB into a buffer, and that buffer needs its own idle inputs
//!   neutralised. Those become extra sinks on the constant nets, which are
//!   routed last in the iteration, so no outer loop is needed.
//! * **Some edges are not routes.** `_op -> _reg` crosses the flip-flop and
//!   carries a different signal; the carry chain is produced and consumed by
//!   the cells themselves. Both exist in the graph for the loop checker, and
//!   both are refused here.
//! * **Lane 3 is a register enable.** Nothing special happens in this file
//!   for that — an enable is a sink like any other, and the single-occupancy
//!   rule is what stops a passing signal from quietly disabling a register.

use crate::fabric::Fabric;
use crate::genlib::{CellLibrary, PhysIn};
use crate::rrg::{Edge, EdgeKind, Node, Rrg};
use std::collections::{BTreeMap, BinaryHeap};

/// What a net is for, which decides how it is reported and ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKind {
    Signal,
    Constant(bool),
    Clock,
}

#[derive(Debug, Clone)]
pub struct Sink {
    /// Human-readable, for the failure report.
    pub what: String,
    /// Any one of these nodes satisfies the sink. More than one means a
    /// genuine choice, such as which ring lane a clock is tapped from.
    pub options: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Net {
    pub name: String,
    pub kind: NetKind,
    /// Nodes the net already occupies at no cost: its driver, and for a
    /// constant every segment the IO map supplies it on.
    pub roots: Vec<usize>,
    pub sinks: Vec<Sink>,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub nets: Vec<Net>,
    /// `reserved[node] = Some(net)` means only that net may use the node —
    /// a placed cell owns its own operation core and its operand pins.
    pub reserved: Vec<Option<usize>>,
    /// CLBs a placed cell occupies, indexed `row * columns + col`. A buffer
    /// can only be built where this is false.
    pub clb_used: Vec<bool>,
    /// The net carrying each constant value, so a buffer discovered during
    /// routing can add its neutralising constant as a sink on the right net.
    pub const_net: [Option<usize>; 2],
}

impl Problem {
    fn clb_is_used(&self, columns: usize, col: usize, row: usize) -> bool {
        self.clb_used.get(row * columns + col).copied().unwrap_or(true)
    }
}

/// One net's routing: the nodes it holds and how it got to each.
#[derive(Debug, Clone, Default)]
pub struct NetRoute {
    pub nodes: Vec<usize>,
    /// `(from, edge)` pairs, in the order they were committed.
    pub edges: Vec<(usize, Edge)>,
}

/// A CLB the router spent on buffering, and how the signal passes through it.
#[derive(Debug, Clone)]
pub struct Buffer {
    pub col: usize,
    pub row: usize,
    /// Physical input the signal arrives on.
    pub input: usize,
    /// Net being buffered.
    pub net: usize,
    /// Chosen implementation of the buffer cell.
    pub op_code: u64,
    pub op_name: String,
    pub inputs: Vec<PhysIn>,
}

/// A sink the router could not reach.
#[derive(Debug, Clone)]
pub struct Unreached {
    pub net: usize,
    pub net_name: String,
    pub what: String,
}

#[derive(Debug, Clone)]
pub struct Routing {
    pub routes: Vec<NetRoute>,
    pub buffers: Vec<Buffer>,
    /// Sinks that could not be reached, each carrying its own description so
    /// the failure report can name it — including sinks discovered while
    /// routing, which are not in the original problem.
    pub unrouted: Vec<Unreached>,
    pub iterations: usize,
    /// Segments wanted by more than one net when the router gave up.
    pub congested: Vec<(Node, u32)>,
}

impl Routing {
    /// Complete means every sink reached **and** nothing left overused.
    ///
    /// Congestion matters as much as reachability: two nets sharing a segment is
    /// not a near miss, it is two nets asking one mux for different codes. Left
    /// out of this check, such a routing looks like a success and only fails
    /// later when the emitter refuses to write conflicting configuration.
    pub fn is_complete(&self) -> bool {
        self.unrouted.is_empty() && self.congested.is_empty()
    }

    /// Board wiring the routing depends on, as `(output pad, input pad, net)`.
    ///
    /// This is what has to physically exist for the bitstream to work, so it is
    /// reported to the user and written into the design file. It is also what
    /// the loop checker needs: a board wire is combinational, so a cycle
    /// through one is a real cycle.
    pub fn loopbacks(&self, rrg: &Rrg) -> Vec<(Node, Node, usize)> {
        let mut out = Vec::new();
        for (net, route) in self.routes.iter().enumerate() {
            for (from, edge) in &route.edges {
                if edge.kind == EdgeKind::Loopback {
                    out.push((rrg.node(*from), rrg.node(edge.to), net));
                }
            }
        }
        out.sort_by_key(|(a, b, net)| (*a, *b, *net));
        out.dedup();
        out
    }
}

/// Base cost of taking an edge. A pass-through is a wire; the operation core
/// is a whole CLB, and is priced to match.
fn edge_base(kind: EdgeKind) -> f64 {
    match kind {
        EdgeKind::Wire => 1.0,
        EdgeKind::Drive => 1.0,
        EdgeKind::Through => 120.0,
        // A board wire: no CLB and no configuration, but it consumes a scarce
        // input pad and has to physically exist, so it is far from free.
        EdgeKind::Loopback => 20.0,
        // Neither is a route; `traversable` refuses them.
        EdgeKind::CarryChain | EdgeKind::Register => f64::INFINITY,
    }
}

fn traversable(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Wire | EdgeKind::Drive | EdgeKind::Through | EdgeKind::Loopback)
}

pub struct Router<'a> {
    rrg: &'a Rrg,
    fabric: &'a Fabric,
    library: &'a CellLibrary,
    occupancy: Vec<u32>,
    history: Vec<f64>,
    /// Which net currently holds each node, for sharing within a net.
    holder: Vec<Option<usize>>,
    buffer_at: BTreeMap<(usize, usize), Buffer>,
    /// Sinks discovered while routing, addressed to another net. Rebuilt every
    /// pass, since they belong to whichever buffers that pass allocated.
    extra_sinks: BTreeMap<usize, Vec<Sink>>,
}

impl<'a> Router<'a> {
    pub fn new(rrg: &'a Rrg, fabric: &'a Fabric, library: &'a CellLibrary) -> Router<'a> {
        Router {
            rrg,
            fabric,
            library,
            occupancy: vec![0; rrg.len()],
            history: vec![0.0; rrg.len()],
            holder: vec![None; rrg.len()],
            buffer_at: BTreeMap::new(),
            extra_sinks: BTreeMap::new(),
        }
    }

    /// Route everything, negotiating for shared segments until nothing is
    /// overused or `max_iterations` is spent.
    pub fn route(&mut self, problem: &Problem, max_iterations: usize) -> Routing {
        let mut routes: Vec<NetRoute> = vec![NetRoute::default(); problem.nets.len()];
        let mut present = 0.5f64;
        let mut unrouted = Vec::new();

        // Signals first, constants last: a buffer allocated while routing a
        // signal adds constant sinks, and they must be served in the same
        // iteration.
        let order: Vec<usize> = {
            let mut ids: Vec<usize> = (0..problem.nets.len()).collect();
            ids.sort_by_key(|&i| match problem.nets[i].kind {
                NetKind::Clock => 0,
                NetKind::Signal => 1,
                NetKind::Constant(_) => 2,
            });
            ids
        };

        // Each pass rips up and reroutes one net at a time, against everyone
        // else's *current* routes. That is what makes the negotiation work: a
        // net that is rerouted sees where the others actually are, so a
        // contested segment gets more expensive for whoever needs it least.
        // Clearing every route at the start of a pass instead would leave each
        // net blind to the ones routed after it, and the result oscillates.
        for pass in 1..=max_iterations {
            unrouted.clear();
            // A buffer's neutralising constant is discovered while routing, so
            // it belongs to the routing that allocated the buffer. The sinks are
            // rebuilt each pass, or constants stay pinned to inputs the router
            // has since stopped using and that reads as congestion that never
            // clears.
            self.extra_sinks.clear();
            self.buffer_at.clear();

            for &net_id in &order {
                self.rip_up(net_id, &mut routes);
                let failures = self.route_net(net_id, problem, &mut routes, present);
                unrouted.extend(failures);
            }

            let overused: Vec<usize> =
                (0..self.rrg.len()).filter(|&n| self.occupancy[n] > 1).collect();
            if overused.is_empty() && unrouted.is_empty() {
                return Routing {
                    routes,
                    buffers: self.buffer_at.values().cloned().collect(),
                    unrouted: Vec::new(),
                    iterations: pass,
                    congested: Vec::new(),
                };
            }
            for &node in &overused {
                self.history[node] += 1.0;
            }
            present *= 1.6;
        }

        let congested: Vec<(Node, u32)> = (0..self.rrg.len())
            .filter(|&n| self.occupancy[n] > 1)
            .map(|n| (self.rrg.node(n), self.occupancy[n]))
            .collect();
        Routing {
            routes,
            buffers: self.buffer_at.values().cloned().collect(),
            unrouted: unrouted.clone(),
            iterations: max_iterations,
            congested,
        }
    }

    /// Release everything a net holds, so it can be rerouted from scratch.
    fn rip_up(&mut self, net: usize, routes: &mut [NetRoute]) {
        for &node in &routes[net].nodes {
            self.occupancy[node] = self.occupancy[node].saturating_sub(1);
            if self.holder[node] == Some(net) {
                self.holder[node] = None;
            }
        }
        routes[net] = NetRoute::default();
    }

    /// Route one net against the current state of every other net. Returns the
    /// sinks it could not reach.
    fn route_net(
        &mut self,
        net_id: usize,
        problem: &Problem,
        routes: &mut [NetRoute],
        present: f64,
    ) -> Vec<Unreached> {
        let net = &problem.nets[net_id];
        let mut route = NetRoute::default();
        let mut failures = Vec::new();
        for &root in &net.roots {
            self.claim(root, net_id, &mut route);
        }

        // Owned, because serving one sink can discover more.
        let mut pending: Vec<Sink> = net
            .sinks
            .iter()
            .chain(self.extra_sinks.get(&net_id).into_iter().flatten())
            .cloned()
            .collect();
        let mut index = 0;
        while index < pending.len() {
            let sink = pending[index].clone();
            index += 1;
            if sink.options.iter().any(|&n| route.nodes.contains(&n)) {
                continue; // already reached, often via a shared lane
            }
            match self.search(&route, &sink, net_id, problem, present) {
                Some(path) => {
                    for (value, discovered) in self.commit(&path, net_id, &mut route) {
                        // A buffer's idle input needs a constant, which belongs
                        // to whichever net carries that value; an input sharing
                        // the signal needs this same net, and is served straight
                        // away by appending to this list.
                        let target = match value {
                            Some(v) => problem.const_net[usize::from(v)],
                            None => Some(net_id),
                        };
                        let Some(target) = target else { continue };
                        if target == net_id {
                            if !pending.iter().any(|s| s.options == discovered.options) {
                                pending.push(discovered);
                            }
                            continue;
                        }
                        let already = self
                            .extra_sinks
                            .get(&target)
                            .is_some_and(|s| s.iter().any(|e| e.options == discovered.options));
                        if !already {
                            self.extra_sinks.entry(target).or_default().push(discovered);
                        }
                    }
                }
                None => failures.push(Unreached {
                    net: net_id,
                    net_name: net.name.clone(),
                    what: sink.what.clone(),
                }),
            }
        }
        routes[net_id] = route;
        failures
    }

    /// Record that a net occupies a node.
    ///
    /// Counted once per net, not once per claim: a net with several sinks
    /// reaches the same segment repeatedly, and counting each visit would make
    /// its own tree look like congestion — which both misreports the conflict
    /// and misleads the negotiation.
    fn claim(&mut self, node: usize, net: usize, route: &mut NetRoute) {
        if route.nodes.contains(&node) {
            return;
        }
        if self.holder[node].is_none() {
            self.holder[node] = Some(net);
        }
        self.occupancy[node] += 1;
        route.nodes.push(node);
    }

    /// Dijkstra from everything the net already holds to the nearest option
    /// of one sink. Ties break on node index so the result never depends on
    /// iteration order.
    fn search(
        &self,
        route: &NetRoute,
        sink: &Sink,
        net: usize,
        problem: &Problem,
        present: f64,
    ) -> Option<Vec<(usize, Edge)>> {
        #[derive(PartialEq)]
        struct Step(f64, usize);
        impl Eq for Step {}
        impl Ord for Step {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                // Reversed: BinaryHeap is a max-heap.
                other
                    .0
                    .total_cmp(&self.0)
                    .then_with(|| other.1.cmp(&self.1))
            }
        }
        impl PartialOrd for Step {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut dist: Vec<f64> = vec![f64::INFINITY; self.rrg.len()];
        let mut from: Vec<Option<(usize, Edge)>> = vec![None; self.rrg.len()];
        let mut heap = BinaryHeap::new();
        for &node in &route.nodes {
            if dist[node] > 0.0 {
                dist[node] = 0.0;
                heap.push(Step(0.0, node));
            }
        }
        if heap.is_empty() {
            return None;
        }

        while let Some(Step(cost, node)) = heap.pop() {
            if cost > dist[node] {
                continue;
            }
            if sink.options.contains(&node) {
                // Walk back to a root.
                let mut path = Vec::new();
                let mut current = node;
                while let Some((previous, edge)) = from[current] {
                    path.push((previous, edge));
                    current = previous;
                }
                path.reverse();
                return Some(path);
            }
            for edge in &self.rrg.out[node] {
                if !traversable(edge.kind) {
                    continue;
                }
                let predecessor = from[node].map(|(p, _)| self.rrg.node(p));
                if !self.usable(edge, node, net, problem, predecessor) {
                    continue;
                }
                let step = self.step_cost(edge, node, net, present, predecessor, route);
                let next = cost + step;
                if next < dist[edge.to] {
                    dist[edge.to] = next;
                    from[edge.to] = Some((node, *edge));
                    heap.push(Step(next, edge.to));
                }
            }
        }
        None
    }

    /// Whether this net may take an edge at all.
    fn usable(
        &self,
        edge: &Edge,
        from: usize,
        net: usize,
        problem: &Problem,
        predecessor: Option<Node>,
    ) -> bool {
        if let Some(owner) = problem.reserved[edge.to] {
            if owner != net {
                return false;
            }
        }
        if edge.kind == EdgeKind::Through {
            // Spending a CLB as a buffer is only possible where no cell is
            // placed, and only if a buffer implementation exists for the
            // input the signal arrived on.
            let Node::Op { col, row } = self.rrg.node(edge.to) else { return false };
            if problem.clb_is_used(self.rrg.columns, col, row) {
                return false;
            }
            let Node::In { mux, .. } = self.rrg.node(from) else { return false };
            // A buffer is only usable if its idle inputs can actually be
            // neutralised *here*. At column 0 no upstream CLB exists to drive
            // a constant onto a major lane, so most forms are unavailable.
            return self
                .buffer_impl(mux, col, row, self.arriving_lane(predecessor))
                .is_some();
        }
        true
    }

    fn step_cost(
        &self,
        edge: &Edge,
        from: usize,
        net: usize,
        present: f64,
        predecessor: Option<Node>,
        own: &NetRoute,
    ) -> f64 {
        let target = edge.to;
        let mut base = edge_base(edge.kind);
        if edge.kind == EdgeKind::Through {
            // Charge the buffer's own constants, so a form that needs none is
            // preferred over one that competes for a major lane.
            if let (Node::In { mux, .. }, Node::Op { col, row }) =
                (self.rrg.node(from), self.rrg.node(target))
            {
                if let Some(imp) = self.buffer_impl(mux, col, row, self.arriving_lane(predecessor))
                {
                    base += 4.0 * imp.constants().count() as f64;
                    // A form that drives several inputs from one signal needs
                    // that signal on more than one lane.
                    let shared = imp
                        .inputs
                        .iter()
                        .enumerate()
                        .filter(|(i, p)| *i != mux && matches!(p, PhysIn::Pin(_)))
                        .count();
                    base += 6.0 * shared as f64;
                }
            }
        }
        // Sharing within a net is free: that is what makes one constant lane
        // serve many sinks.
        if own.nodes.contains(&target) {
            return 0.0;
        }
        // Nodes this net does not already hold are charged for whoever else is
        // on them.
        let overuse = self.occupancy[target] as f64;
        let _ = net;
        (base + self.history[target]) * (1.0 + present * overuse)
    }

    /// Cheapest buffer implementation whose live pin sits on `phys_input` and
    /// whose tied inputs can be given a constant at this position.
    ///
    /// The position check is what stops the router allocating a buffer in
    /// column 0, where no upstream CLB exists to drive a constant onto a major
    /// lane and the buffer would be unroutable.
    fn buffer_impl(
        &self,
        phys_input: usize,
        col: usize,
        row: usize,
        arriving_lane: Option<usize>,
    ) -> Option<&crate::genlib::CellImpl> {
        let buf = self
            .library
            .cells
            .iter()
            .find(|c| c.arity == 1 && c.table == [false, true])?;
        buf.impls_for_input(phys_input).into_iter().find(|imp| {
            let constants_ok = imp
                .constants()
                .all(|(input, value)| self.fabric.constant_reaches_input(input, col, row, value));
            // A form that drives several physical inputs from one signal is
            // only usable when a single lane segment can feed all of them,
            // since that is what the signal actually arrives on. `MUX2(p, p, -)`
            // qualifies because inputs `a` and `b` overlap on one lane — and it
            // is the only buffer needing no constant, so it is the only way to
            // buffer where no constant can be reached, such as column 0 outside
            // the row whose edge supplies constants.
            constants_ok && self.shared_pins_reachable(imp, arriving_lane)
        })
    }

    /// Whether the inputs an implementation drives from one signal can all be
    /// fed by the segment the signal actually arrived on.
    ///
    /// The arrival lane matters, not just the mux window: a signal reaching
    /// input `c` from the vertical ring is on no horizontal lane at all, so
    /// nothing can share it, even though `c` and `b` do overlap on a lane in
    /// general.
    fn shared_pins_reachable(
        &self,
        imp: &crate::genlib::CellImpl,
        arriving_lane: Option<usize>,
    ) -> bool {
        let driven = imp.inputs.iter().filter(|p| matches!(p, PhysIn::Pin(_))).count();
        if driven <= 1 {
            return true;
        }
        // Several inputs share the signal, so it has to be on a horizontal
        // lane every one of them can select.
        let Some(lane) = arriving_lane else { return false };
        imp.inputs.iter().enumerate().all(|(input, phys)| {
            !matches!(phys, PhysIn::Pin(_))
                || self.fabric.input_muxes[input]
                    .sources
                    .contains(&crate::fabric::Source::HorzIn(lane))
        })
    }

    /// The horizontal lane a signal is on, if it arrived on one.
    fn arriving_lane(&self, predecessor: Option<Node>) -> Option<usize> {
        match predecessor {
            Some(Node::HSeg { lane, .. }) => Some(lane),
            _ => None,
        }
    }

    /// Take a path, claiming its nodes. Returns constant sinks that any
    /// buffer on the path now needs.
    fn commit(
        &mut self,
        path: &[(usize, Edge)],
        net: usize,
        route: &mut NetRoute,
    ) -> Vec<(Option<bool>, Sink)> {
        let mut discovered = Vec::new();
        for (step, &(from, edge)) in path.iter().enumerate() {
            let predecessor = step.checked_sub(1).map(|p| self.rrg.node(path[p].0));
            self.claim(from, net, route);
            self.claim(edge.to, net, route);
            route.edges.push((from, edge));
            if edge.kind == EdgeKind::Through {
                if let (Node::Op { col, row }, Node::In { mux, .. }) =
                    (self.rrg.node(edge.to), self.rrg.node(from))
                {
                    if let Some(imp) =
                        self.buffer_impl(mux, col, row, self.arriving_lane(predecessor)).cloned()
                    {
                        for (input, value) in imp.constants() {
                            if let Some(id) = self.rrg.id(Node::In { col, row, mux: input }) {
                                discovered.push((
                                    Some(value),
                                    Sink {
                                        what: format!(
                                            "buffer at CLB({}, {}) input {}",
                                            col,
                                            row,
                                            self.phys_name(input)
                                        ),
                                        options: vec![id],
                                    },
                                ));
                            }
                        }
                        // Some buffer forms drive several physical inputs from
                        // the same signal — `MUX2(p, p, -)` is the only way to
                        // buffer where no constant can be reached. Those inputs
                        // are extra sinks on the *same* net, and leaving them
                        // out would quietly compute the wrong function.
                        for (input, phys) in imp.inputs.iter().enumerate() {
                            if input == mux || !matches!(phys, PhysIn::Pin(_)) {
                                continue;
                            }
                            if let Some(id) = self.rrg.id(Node::In { col, row, mux: input }) {
                                discovered.push((
                                    None,
                                    Sink {
                                        what: format!(
                                            "buffer at CLB({}, {}) input {} (shared with {})",
                                            col,
                                            row,
                                            self.phys_name(input),
                                            self.phys_name(mux)
                                        ),
                                        options: vec![id],
                                    },
                                ));
                            }
                        }
                        self.buffer_at.insert(
                            (col, row),
                            Buffer {
                                col,
                                row,
                                input: mux,
                                net,
                                op_code: imp.op_code,
                                op_name: imp.op_name.clone(),
                                inputs: imp.inputs.clone(),
                            },
                        );
                    }
                }
            }
        }
        discovered
    }

    fn phys_name(&self, input: usize) -> String {
        self.library
            .phys_names
            .get(input)
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }

    pub fn fabric(&self) -> &Fabric {
        self.fabric
    }
}
