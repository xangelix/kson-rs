use crate::*;

#[derive(Debug, Copy, Clone)]
pub enum ScoreTick {
    Laser { lane: usize, pos: f64 },
    Slam { lane: usize, start: f64, end: f64 },
    Chip { lane: usize },
    Hold { lane: usize },
}

#[derive(Debug, Copy, Clone)]
pub struct PlacedScoreTick {
    pub y: u32,
    pub tick: ScoreTick,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ScoreTickSummary {
    pub chip_count: u32,
    pub hold_count: u32,
    pub laser_count: u32,
    pub slam_count: u32,
    pub total: u32,
}

pub trait ScoreTicker {
    fn summary(&self) -> ScoreTickSummary;
    fn get_combo_at(&self, y: u32) -> u32;
}

fn get_hold_step_at(y: u32, chart: &Chart) -> u32 {
    if chart.bpm_at_tick(y) > 255.0 {
        chart.beat.resolution / 2
    } else {
        chart.beat.resolution / 4
    }
}

fn ticks_from_interval(interval: &Interval, lane: usize, chart: &Chart) -> Vec<PlacedScoreTick> {
    if interval.l == 0 {
        vec![PlacedScoreTick {
            y: interval.y,
            tick: ScoreTick::Chip { lane },
        }]
    } else {
        let mut res = Vec::new();

        let mut y = interval.y;
        let mut step = get_hold_step_at(y, chart);
        y += step;
        y -= y % step;
        while y <= interval.y + interval.l - step {
            res.push(PlacedScoreTick {
                y,
                tick: ScoreTick::Hold { lane },
            });
            step = get_hold_step_at(y, chart);
            y += step;
        }

        res
    }
}

fn get_if_slam(point: Option<&GraphSectionPoint>, lane: usize, y: u32) -> Option<PlacedScoreTick> {
    if let Some(s) = point {
        if let Some(vf) = s.vf {
            Some(PlacedScoreTick {
                y: y + s.ry,
                tick: ScoreTick::Slam {
                    lane,
                    end: vf,
                    start: s.v,
                },
            })
        } else {
            None
        }
    } else {
        None
    }
}

fn ticks_from_laser_section(
    section: &LaserSection,
    lane: usize,
    chart: &Chart,
) -> Vec<PlacedScoreTick> {
    let mut res = Vec::new();

    let mut first = true;
    for se in section.v.windows(2) {
        let s = se[0];
        let e = se[1];
        if let Some(t) = get_if_slam(Some(&s), lane, section.y) {
            res.push(t)
        }

        let mut y = section.y + s.ry;
        let mut step = get_hold_step_at(y, chart);
        if s.vf.is_some() || first {
            y += step;
        }
        y -= y % step;
        while y <= section.y + e.ry - step {
            if match res.last() {
                Some(s) => s.y == y,
                None => false,
            } {
                step = get_hold_step_at(y, chart);
                y += step;
                continue;
            }

            res.push(PlacedScoreTick {
                y,
                tick: ScoreTick::Laser {
                    lane,
                    pos: section.value_at(y as f64).unwrap_or_default(),
                },
            });
            step = get_hold_step_at(y, chart);
            y += step;
        }
        first = false;
    }

    if let Some(t) = get_if_slam(section.v.last(), lane, section.y) {
        res.push(t);
    }

    res
}

type ScoreTicks = Vec<PlacedScoreTick>;

pub fn generate_score_ticks(chart: &Chart) -> ScoreTicks {
    let mut res = Vec::new();

    res.append(
        &mut chart
            .note
            .bt
            .iter()
            .enumerate()
            .map(|(lane, l)| l.iter().map(move |i| ticks_from_interval(i, lane, chart)))
            .flatten()
            .flatten()
            .collect(),
    );
    res.append(
        &mut chart
            .note
            .fx
            .iter()
            .enumerate()
            .map(|(lane, l)| l.iter().map(move |i| ticks_from_interval(i, lane, chart)))
            .flatten()
            .flatten()
            .collect(),
    );
    res.append(
        &mut chart
            .note
            .laser
            .iter()
            .enumerate()
            .map(|(lane, l)| {
                l.iter()
                    .map(move |s| ticks_from_laser_section(s, lane, chart))
            })
            .flatten()
            .flatten()
            .collect(),
    );

    res.sort_by(|pa, pb| pa.y.cmp(&pb.y));

    res
}

impl ScoreTicker for ScoreTicks {
    fn summary(&self) -> ScoreTickSummary {
        let mut res: ScoreTickSummary = Default::default();

        for t in self {
            res.total += 1;
            match t.tick {
                ScoreTick::Laser { lane: _, pos: _ } => res.laser_count += 1,
                ScoreTick::Slam {
                    lane: _,
                    start: _,
                    end: _,
                } => res.slam_count += 1,
                ScoreTick::Chip { lane: _ } => res.chip_count += 1,
                ScoreTick::Hold { lane: _ } => res.hold_count += 1,
            }
        }

        res
    }

    fn get_combo_at(&self, y: u32) -> u32 {
        match self.binary_search_by(|f| f.y.cmp(&y)) {
            Ok(c) => c as u32 + 1,
            Err(c) => c as u32,
        }
    }
}
