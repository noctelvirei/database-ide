//! PROTOTYPE. One operator tree for both engines, so every plan view draws SQL Server and
//! PostgreSQL plans the same way. Built by `showplan.rs` and `pg_json.rs`; rough on purpose.

/// Which database engine produced a plan.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Engine {
    SqlServer,
    Postgres,
}

/// One operator of an actual plan. Rows are totals over all executions, so estimated and
/// actual compare directly even on the inner side of a nested loop.
#[derive(Debug)]
pub struct Op {
    /// Physical operator, e.g. "Hash Match" or "Seq Scan".
    pub name: String,
    /// Logical operator or join type, e.g. "Right Anti Semi Join"; may be empty.
    pub logical: String,
    /// Table or index the operator reads, e.g. "orders.orders_customer_ix"; may be empty.
    pub object: String,
    /// Name of the icon file in `icons/` (without `.svg`).
    pub icon: &'static str,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Estimated rows over all estimated executions.
    pub est_rows: f64,
    pub act_rows: f64,
    pub executions: f64,
    /// Estimated cost of this operator alone, as a share of the plan's total, 0..=100.
    pub cost_pct: f64,
    /// Estimated cost of this operator and everything under it, 0..=100.
    pub subtree_cost_pct: f64,
    /// Actual elapsed milliseconds of this operator alone (clamped at 0).
    pub self_ms: f64,
    /// Actual elapsed milliseconds including children.
    pub total_ms: f64,
    /// Logical reads (SQL Server pages, PostgreSQL shared blocks hit + read) of this operator alone.
    pub reads: f64,
    /// Whether the operator ran in parallel.
    pub parallel: bool,
    /// Engine warnings on this operator, e.g. "Spill to tempdb".
    pub warnings: Vec<String>,
    /// Extra facts for the inspector: predicates, sort keys, memory and so on.
    pub details: Vec<(String, String)>,
}

impl Op {
    /// How far the estimate is off, as a factor >= 1, with `true` when actual exceeds the
    /// estimate. Rows under 1 count as 1 so empty results don't divide by zero.
    pub fn misestimate(&self) -> (f64, bool) {
        let (est, act) = (self.est_rows.max(1.), self.act_rows.max(1.));
        if act >= est {
            (act / est, true)
        } else {
            (est / act, false)
        }
    }

    /// True when the estimate is off by 10x or more either way.
    pub fn is_misestimated(&self) -> bool {
        self.misestimate().0 >= 10.
    }

    /// Short label for the operator: physical name plus logical name when it adds something.
    pub fn label(&self) -> String {
        if self.logical.is_empty() || self.logical == self.name {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.logical)
        }
    }
}

/// A parsed actual plan for one statement. Operator 0 is the root.
#[derive(Debug)]
pub struct Plan {
    pub engine: Engine,
    /// Short name shown in the pill and tab list.
    pub title: &'static str,
    pub statement: String,
    pub ops: Vec<Op>,
    /// Whole statement's elapsed time in milliseconds.
    pub elapsed_ms: f64,
    /// Plan-level facts for the inspector header, e.g. memory grant or planning time.
    pub facts: Vec<(String, String)>,
}

impl Plan {
    /// Fills `children`, `cost_pct`, `self_ms` and `reads` (made exclusive) from the
    /// inclusive values the parsers stored. `subtree` holds each operator's inclusive
    /// estimated cost; `reads` arrive inclusive for PostgreSQL only (`inclusive_reads`).
    pub fn finish(&mut self, subtree: &[f64], inclusive_reads: bool) {
        for i in 0..self.ops.len() {
            if let Some(p) = self.ops[i].parent {
                self.ops[p].children.push(i);
            }
        }
        let total = subtree
            .first()
            .copied()
            .unwrap_or(1.)
            .max(f64::MIN_POSITIVE);
        // Snapshot inclusive reads first so each subtraction uses children's inclusive values.
        let inclusive_reads_of: Vec<f64> = self.ops.iter().map(|o| o.reads).collect();
        for i in 0..self.ops.len() {
            let kids = self.ops[i].children.clone();
            let child_cost: f64 = kids.iter().map(|&c| subtree[c]).sum();
            let child_ms = kids
                .iter()
                .map(|&c| self.ops[c].total_ms)
                .fold(0., f64::max);
            let child_reads: f64 = kids.iter().map(|&c| inclusive_reads_of[c]).sum();
            let op = &mut self.ops[i];
            op.subtree_cost_pct = 100. * subtree[i] / total;
            op.cost_pct = (100. * (subtree[i] - child_cost) / total).max(0.);
            op.self_ms = (op.total_ms - child_ms).max(0.);
            if inclusive_reads {
                op.reads = (inclusive_reads_of[i] - child_reads).max(0.);
            }
        }
    }
}

/// Formats a row count compactly: 1234 -> "1,234", 24534730 -> "24.5M".
pub fn rows(n: f64) -> String {
    if n >= 10_000_000. {
        format!("{:.1}M", n / 1_000_000.)
    } else if n >= 100_000. {
        format!("{:.0}k", n / 1_000.)
    } else if n >= 1. || n == 0. {
        let s = format!("{:.0}", n);
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        out
    } else {
        format!("{n:.2}")
    }
}

/// Formats how far an estimate is off, with an arrow for the direction: "▲ 20×", "▼ 24M×".
pub fn factor((factor, over): (f64, bool)) -> String {
    let arrow = if factor < 1.5 {
        ""
    } else if over {
        "▲ "
    } else {
        "▼ "
    };
    format!("{arrow}{}×", rows(factor.round()))
}

/// Formats milliseconds: "820 ms", "12.4 s", "26 min".
pub fn ms(v: f64) -> String {
    if v >= 120_000. {
        format!("{:.0} min", v / 60_000.)
    } else if v >= 1_000. {
        format!("{:.1} s", v / 1_000.)
    } else if v >= 10. {
        format!("{v:.0} ms")
    } else {
        format!("{v:.2} ms")
    }
}
