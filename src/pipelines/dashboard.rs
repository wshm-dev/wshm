use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::DashboardArgs;
use crate::db::Database;

/// A single snapshot of repo metrics at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub timestamp: String,
    pub open_issues: usize,
    pub untriaged: usize,
    pub open_prs: usize,
    pub unanalyzed: usize,
    pub conflicts: usize,
    pub avg_pr_age_days: f64,
    pub avg_issue_age_days: f64,
    pub prs_over_7d: usize,
    pub prs_over_30d: usize,
    pub issues_over_7d: usize,
    pub issues_over_30d: usize,
}

pub fn run(db: &Database, args: &DashboardArgs) -> Result<()> {
    // Step 1: Take a snapshot of current metrics
    let snapshot = take_snapshot(db)?;

    // Step 2: Store snapshot in DB
    store_snapshot(db, &snapshot)?;

    // Step 3: Load all historical snapshots
    let history = load_history(db)?;

    // Step 4: Generate dashboard HTML
    let html = render_dashboard(&history);

    let output = args
        .output
        .clone()
        .unwrap_or_else(|| "wshm-dashboard.html".to_string());

    std::fs::write(&output, &html)
        .with_context(|| format!("Failed to write {output}"))?;

    println!("Dashboard written to {output} ({} snapshots)", history.len());
    Ok(())
}

fn take_snapshot(db: &Database) -> Result<MetricSnapshot> {
    let open_issues = db.get_open_issues()?;
    let untriaged = db.get_untriaged_issues()?;
    let open_prs = db.get_open_pulls()?;
    let unanalyzed = db.get_unanalyzed_pulls()?;
    let now = chrono::Utc::now();

    let conflicts = open_prs
        .iter()
        .filter(|p| p.mergeable == Some(false))
        .count();

    let pr_ages: Vec<i64> = open_prs
        .iter()
        .filter_map(|pr| {
            pr.created_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .ok()
                .map(|dt| now.signed_duration_since(dt).num_days())
        })
        .collect();

    let issue_ages: Vec<i64> = open_issues
        .iter()
        .filter_map(|i| {
            i.created_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .ok()
                .map(|dt| now.signed_duration_since(dt).num_days())
        })
        .collect();

    let avg_pr = if pr_ages.is_empty() {
        0.0
    } else {
        pr_ages.iter().sum::<i64>() as f64 / pr_ages.len() as f64
    };
    let avg_issue = if issue_ages.is_empty() {
        0.0
    } else {
        issue_ages.iter().sum::<i64>() as f64 / issue_ages.len() as f64
    };

    Ok(MetricSnapshot {
        timestamp: now.to_rfc3339(),
        open_issues: open_issues.len(),
        untriaged: untriaged.len(),
        open_prs: open_prs.len(),
        unanalyzed: unanalyzed.len(),
        conflicts,
        avg_pr_age_days: avg_pr,
        avg_issue_age_days: avg_issue,
        prs_over_7d: pr_ages.iter().filter(|&&d| d > 7).count(),
        prs_over_30d: pr_ages.iter().filter(|&&d| d > 30).count(),
        issues_over_7d: issue_ages.iter().filter(|&&d| d > 7).count(),
        issues_over_30d: issue_ages.iter().filter(|&&d| d > 30).count(),
    })
}

fn store_snapshot(db: &Database, snapshot: &MetricSnapshot) -> Result<()> {
    db.with_conn(|conn| {
        // Ensure table exists
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metric_snapshots (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT NOT NULL,
                data        TEXT NOT NULL
            );",
        )?;

        let data = serde_json::to_string(snapshot)?;
        conn.execute(
            "INSERT INTO metric_snapshots (timestamp, data) VALUES (?1, ?2)",
            rusqlite::params![snapshot.timestamp, data],
        )?;
        Ok(())
    })
}

fn load_history(db: &Database) -> Result<Vec<MetricSnapshot>> {
    db.with_conn(|conn| {
        // Table might not exist yet
        let exists: bool = conn
            .prepare("SELECT 1 FROM metric_snapshots LIMIT 0")
            .is_ok();
        if !exists {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            "SELECT data FROM metric_snapshots ORDER BY timestamp ASC",
        )?;
        let snapshots = stmt
            .query_map([], |row| {
                let data: String = row.get(0)?;
                Ok(data)
            })?
            .filter_map(|r| r.ok())
            .filter_map(|data| serde_json::from_str::<MetricSnapshot>(&data).ok())
            .collect();
        Ok(snapshots)
    })
}

fn render_dashboard(history: &[MetricSnapshot]) -> String {
    let json_data = serde_json::to_string(history).unwrap_or_else(|_| "[]".to_string());

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>wshm Dashboard</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4"></script>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; max-width: 1400px; margin: 0 auto; padding: 2rem; background: #fafbfc; color: #1a1a2e; }}
  h1 {{ font-size: 1.8rem; margin-bottom: 0.3rem; }}
  .meta {{ color: #586069; font-size: 0.9rem; margin-bottom: 2rem; }}
  .overview {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 1rem; margin-bottom: 2rem; }}
  .card {{ background: white; border: 1px solid #e1e4e8; border-radius: 8px; padding: 1rem; text-align: center; }}
  .card .number {{ font-size: 2rem; font-weight: 700; }}
  .card .label {{ font-size: 0.8rem; color: #586069; text-transform: uppercase; letter-spacing: 0.05em; }}
  .card .trend {{ font-size: 0.8rem; margin-top: 0.3rem; }}
  .trend-up {{ color: #b31d28; }}
  .trend-down {{ color: #22863a; }}
  .trend-flat {{ color: #586069; }}
  .charts {{ display: grid; grid-template-columns: 1fr 1fr; gap: 2rem; margin-top: 2rem; }}
  .chart-container {{ background: white; border: 1px solid #e1e4e8; border-radius: 8px; padding: 1.5rem; }}
  .chart-container h2 {{ font-size: 1.1rem; margin-bottom: 1rem; }}
  @media (max-width: 768px) {{ .charts {{ grid-template-columns: 1fr; }} }}
  .footer {{ margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #e1e4e8; font-size: 0.8rem; color: #586069; }}
</style>
</head>
<body>
<h1>wshm Dashboard</h1>
<p class="meta" id="meta"></p>

<div class="overview" id="overview"></div>

<div class="charts">
  <div class="chart-container">
    <h2>Issues & PRs Over Time</h2>
    <canvas id="chart-counts"></canvas>
  </div>
  <div class="chart-container">
    <h2>Average Age (days)</h2>
    <canvas id="chart-age"></canvas>
  </div>
  <div class="chart-container">
    <h2>SLA: Items > 7 days</h2>
    <canvas id="chart-sla7"></canvas>
  </div>
  <div class="chart-container">
    <h2>Conflicts & Untriaged</h2>
    <canvas id="chart-health"></canvas>
  </div>
</div>

<div class="footer">Generated by <a href="https://github.com/pszymkowiak/wshm">wshm</a></div>

<script>
const history = {json_data};

if (history.length === 0) {{
  document.getElementById('meta').textContent = 'No data yet. Run `wshm dashboard` after syncing.';
}} else {{
  const latest = history[history.length - 1];
  const prev = history.length > 1 ? history[history.length - 2] : null;
  const ts = new Date(latest.timestamp).toLocaleString();
  document.getElementById('meta').textContent = `Last updated: ${{ts}} — ${{history.length}} snapshots`;

  // Overview cards
  const metrics = [
    {{ label: 'Open Issues', value: latest.open_issues, prev: prev?.open_issues }},
    {{ label: 'Untriaged', value: latest.untriaged, prev: prev?.untriaged }},
    {{ label: 'Open PRs', value: latest.open_prs, prev: prev?.open_prs }},
    {{ label: 'Conflicts', value: latest.conflicts, prev: prev?.conflicts }},
    {{ label: 'Avg PR Age', value: latest.avg_pr_age_days.toFixed(1) + 'd', prev: prev?.avg_pr_age_days }},
    {{ label: 'PRs > 30d', value: latest.prs_over_30d, prev: prev?.prs_over_30d }},
  ];

  const overview = document.getElementById('overview');
  metrics.forEach(m => {{
    let trend = '';
    if (prev && typeof m.prev === 'number') {{
      const num = typeof m.value === 'string' ? parseFloat(m.value) : m.value;
      const diff = num - m.prev;
      if (diff > 0) trend = `<div class="trend trend-up">↑ +${{diff.toFixed?.(1) || diff}}</div>`;
      else if (diff < 0) trend = `<div class="trend trend-down">↓ ${{diff.toFixed?.(1) || diff}}</div>`;
      else trend = `<div class="trend trend-flat">→ stable</div>`;
    }}
    overview.innerHTML += `<div class="card"><div class="number">${{m.value}}</div><div class="label">${{m.label}}</div>${{trend}}</div>`;
  }});

  // Charts
  const labels = history.map(h => {{
    const d = new Date(h.timestamp);
    return `${{d.getMonth()+1}}/${{d.getDate()}}`;
  }});

  const chartOpts = {{ responsive: true, plugins: {{ legend: {{ position: 'bottom' }} }}, scales: {{ y: {{ beginAtZero: true }} }} }};

  new Chart(document.getElementById('chart-counts'), {{
    type: 'line',
    data: {{
      labels,
      datasets: [
        {{ label: 'Open Issues', data: history.map(h => h.open_issues), borderColor: '#0366d6', tension: 0.3 }},
        {{ label: 'Open PRs', data: history.map(h => h.open_prs), borderColor: '#22863a', tension: 0.3 }},
      ]
    }},
    options: chartOpts
  }});

  new Chart(document.getElementById('chart-age'), {{
    type: 'line',
    data: {{
      labels,
      datasets: [
        {{ label: 'Avg PR Age', data: history.map(h => h.avg_pr_age_days.toFixed(1)), borderColor: '#0366d6', tension: 0.3 }},
        {{ label: 'Avg Issue Age', data: history.map(h => h.avg_issue_age_days.toFixed(1)), borderColor: '#e36209', tension: 0.3 }},
      ]
    }},
    options: chartOpts
  }});

  new Chart(document.getElementById('chart-sla7'), {{
    type: 'bar',
    data: {{
      labels,
      datasets: [
        {{ label: 'PRs > 7d', data: history.map(h => h.prs_over_7d), backgroundColor: '#0366d6' }},
        {{ label: 'Issues > 7d', data: history.map(h => h.issues_over_7d), backgroundColor: '#e36209' }},
      ]
    }},
    options: chartOpts
  }});

  new Chart(document.getElementById('chart-health'), {{
    type: 'line',
    data: {{
      labels,
      datasets: [
        {{ label: 'Conflicts', data: history.map(h => h.conflicts), borderColor: '#b31d28', tension: 0.3 }},
        {{ label: 'Untriaged', data: history.map(h => h.untriaged), borderColor: '#735c0f', tension: 0.3 }},
      ]
    }},
    options: chartOpts
  }});
}}
</script>
</body>
</html>"#
    )
}
