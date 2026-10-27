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
//! * **Constants are nets, not annotations.** A register enable held at 1, or
//!   an unused DDIO's direction held at 0, is a sink on one shared constant
//!   net per value, driven from the major muxes, so several sinks share a lane
//!   wherever the hardware allows it.
//! * **Taking a LUT as a pass-through turns a spare CLB into a buffer.** The
//!   LUT is set to copy whichever input the signal arrived on and ignore the
//!   other two, so a buffer needs nothing but the CLB itself.
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
    /// Nodes the net already occupies at no cost: its driver.
    pub roots: Vec<usize>,
    pub sinks: Vec<Sink>,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub nets: Vec<Net>,
    /// `reserved[node] = Some(net)` means only that net may use the node —
    /// a placed cell owns its own LUT and its operand pins.
    pub reserved: Vec<Option<usize>>,
    /// CLBs a placed cell occupies, indexed `row * columns + col`. A buffer
    /// can only be built where this is false.
    pub clb_used: Vec<bool>,
    /// The net carrying each constant value.
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
    /// The LUT value that copies `input` through.
    pub lut: u64,
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

    /// On-chip loops the routing uses, as `(output pad node, left-edge
    /// segment, net)`: chip outputs the IO controller reads back.
    pub fn on_chip_loops(&self, rrg: &Rrg) -> Vec<(Node, Node, usize)> {
        let mut out = Vec::new();
        for (net, route) in self.routes.iter().enumerate() {
            for (from, edge) in &route.edges {
                if edge.kind == EdgeKind::OnChipLoop {
                    out.push((rrg.node(*from), rrg.node(edge.to), net));
                }
            }
        }
        out.sort_by_key(|(a, b, net)| (*a, *b, *net));
        out.dedup();
        out
    }
}

/// Base cost of taking an edge. A pass-through is a wire; the LUT
/// is a whole CLB, and is priced to match.
fn edge_base(kind: EdgeKind) -> f64 {
    match kind {
        EdgeKind::Wire => 1.0,
        EdgeKind::Drive => 1.0,
        EdgeKind::Through => 120.0,
        // A board wire: no CLB and no configuration, but it consumes a scarce
        // input pad and has to physically exist, so it is far from free.
        EdgeKind::Loopback => 20.0,
        // Reading a chip output back on-chip spends no CLB and needs no
        // wiring, but there are only a few such outputs and they are shared
        // with design outputs, so it is priced well above a wire.
        EdgeKind::OnChipLoop => 12.0,
        // Neither is a route; `traversable` refuses them.
        EdgeKind::CarryChain | EdgeKind::Register => f64::INFINITY,
    }
}

fn traversable(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Wire | EdgeKind::Drive | EdgeKind::Through | EdgeKind::Loopback | EdgeKind::OnChipLoop
    )
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
        }
    }

    /// Route everything, negotiating for shared segments until nothing is
    /// overused or `max_iterations` is spent.
    pub fn route(&mut self, problem: &Problem, max_iterations: usize) -> Routing {
        let mut routes: Vec<NetRoute> = vec![NetRoute::default(); problem.nets.len()];
        let mut present = 0.5f64;
        let mut unrouted = Vec::new();

        // Clocks first, then signals, then constants.
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
            // Buffers belong to the routing that allocated them, so they are
            // rebuilt each pass.
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

        for sink in &net.sinks {
            if sink.options.iter().any(|&n| route.nodes.contains(&n)) {
                continue; // already reached, often via a shared lane
            }
            match self.search(&route, sink, net_id, problem, present) {
                Some(path) => self.commit(&path, net_id, &mut route),
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
            // placed.
            let Node::Op { col, row } = self.rrg.node(edge.to) else { return false };
            if problem.clb_is_used(self.rrg.columns, col, row) {
                return false;
            }
            let Node::In { mux, .. } = self.rrg.node(from) else { return false };
            let _ = predecessor;
            return self.buffer_impl(mux).is_some();
        }
        true
    }

    fn step_cost(
        &self,
        edge: &Edge,
        _from: usize,
        net: usize,
        present: f64,
        _predecessor: Option<Node>,
        own: &NetRoute,
    ) -> f64 {
        let target = edge.to;
        let base = edge_base(edge.kind);
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

    /// The buffer implementation whose live pin sits on `phys_input`: a LUT
    /// copying that input and ignoring the others.
    fn buffer_impl(&self, phys_input: usize) -> Option<&crate::genlib::CellImpl> {
        self.library.buffer()?.impls_for_input(phys_input).into_iter().next()
    }

    /// Take a path, claiming its nodes and recording any buffer it spends.
    fn commit(&mut self, path: &[(usize, Edge)], net: usize, route: &mut NetRoute) {
        for &(from, edge) in path {
            self.claim(from, net, route);
            self.claim(edge.to, net, route);
            route.edges.push((from, edge));
            if edge.kind == EdgeKind::Through {
                if let (Node::Op { col, row }, Node::In { mux, .. }) =
                    (self.rrg.node(edge.to), self.rrg.node(from))
                {
                    if let Some(imp) = self.buffer_impl(mux).cloned() {
                        self.buffer_at.insert(
                            (col, row),
                            Buffer { col, row, input: mux, net, lut: imp.lut, inputs: imp.inputs },
                        );
                    }
                }
            }
        }
    }

    pub fn fabric(&self) -> &Fabric {
        self.fabric
    }
}
