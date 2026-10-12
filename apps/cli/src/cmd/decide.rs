//! `agentplaza decide`: Ask the plaza's decision model a question.

use serde_json::{json, Value};

use crate::ctx::{self, text_arg, Ctx};

/// One question about `--input`: a yes/no `--predicate`, a `--choice` among `--options`, or a
/// `--score` on `--levels`. The answers are printed as probabilities.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// The text the question is about, or "-" to read it from stdin.
    #[arg(long)]
    input: String,
    /// A yes/no question: the probability that it holds.
    #[arg(long, conflicts_with_all = ["choice", "score"])]
    predicate: Option<String>,
    /// A question with fixed answers (see --options): the probability of each.
    #[arg(long, requires = "options", conflicts_with = "score")]
    choice: Option<String>,
    /// The answers for --choice, comma separated.
    #[arg(long, requires = "choice")]
    options: Option<String>,
    /// A question answered on a scale (see --levels): the probability of each level.
    #[arg(long, requires = "levels")]
    score: Option<String>,
    /// The levels for --score, comma separated, lowest first.
    #[arg(long, requires = "score")]
    levels: Option<String>,
    /// The decision model's provider (default: the plaza's decision model).
    #[arg(long)]
    provider: Option<String>,
    /// The decision model (with --provider).
    #[arg(long)]
    model: Option<String>,
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let input = text_arg(&args.input)?;
    let (question, text) = build_question(&args)?;
    let (provider, model) = decision_model(ctx, args.provider, args.model)?;
    let body = json!({ "provider": provider, "model": model, "input": input, "questions": [question] });
    let response = ctx.client.post("/v1/decisions", &body)?;
    ctx.out(&response, |r| print_answers(r, &text));
    Ok(())
}

/// The one question as the hub takes it, and its text for the printout.
fn build_question(args: &Args) -> Result<(Value, String), String> {
    if let Some(text) = &args.predicate {
        return Ok((json!({ "type": "predicate", "name": "answer", "instructions": text }), text.clone()));
    }
    if let Some(text) = &args.choice {
        let options = split_list(args.options.as_deref().ok_or("--choice needs --options")?)?;
        let choices: Vec<Value> = options.iter().map(|o| json!({ "value": o })).collect();
        return Ok((json!({ "type": "choice", "name": "answer", "instructions": text, "choices": choices }), text.clone()));
    }
    if let Some(text) = &args.score {
        let levels = split_list(args.levels.as_deref().ok_or("--score needs --levels")?)?;
        let levels: Vec<Value> = levels.iter().map(|l| json!({ "label": l })).collect();
        return Ok((json!({ "type": "score", "name": "answer", "instructions": text, "levels": levels }), text.clone()));
    }
    Err("pick one question: --predicate, --choice or --score".into())
}

fn split_list(list: &str) -> Result<Vec<String>, String> {
    let items: Vec<String> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    if items.len() < 2 {
        return Err("give at least two, comma separated".into());
    }
    Ok(items)
}

fn decision_model(ctx: &Ctx, provider: Option<String>, model: Option<String>) -> Result<(String, String), String> {
    match (provider, model) {
        (Some(p), Some(m)) => Ok((p, m)),
        (None, None) => {
            let default = ctx.client.get("/v1/decisions/default")?;
            match (default["provider"].as_str(), default["model"].as_str()) {
                (Some(p), Some(m)) => Ok((p.to_string(), m.to_string())),
                _ => Err("the plaza has no decision model set: pass --provider and --model".into()),
            }
        }
        _ => Err("pass both --provider and --model, or neither".into()),
    }
}

fn print_answers(response: &Value, question: &str) {
    let answers = response["answers"].as_array().cloned().unwrap_or_default();
    if answers.is_empty() {
        println!("(no answer)");
        return;
    }
    println!("{question}");
    for answer in &answers {
        print_answer(answer);
    }
}

fn print_answer(a: &Value) {
    match ctx::s(a, "type") {
        "predicate" => row("holds", prob(a, "probability")),
        "choice" => {
            println!("answer: {} (confidence {:.0}%)", ctx::s(a, "choice"), prob(a, "confidence") * 100.0);
            for p in a["probabilities"].as_array().into_iter().flatten() {
                row(ctx::s(p, "value"), prob(p, "probability"));
            }
        }
        "score" => {
            let score = &a["score"];
            let label = a["probabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["value"] == *score)
                .map(|p| ctx::s(p, "label").to_string())
                .unwrap_or_default();
            println!("score: {} ({label}) (confidence {:.0}%)", ctx::text(a, "score"), prob(a, "confidence") * 100.0);
            for p in a["probabilities"].as_array().into_iter().flatten() {
                row(&format!("{} {}", ctx::text(p, "value"), ctx::s(p, "label")), prob(p, "probability"));
            }
        }
        _ => println!("the model refused to answer"),
    }
}

fn prob(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `  73.0%  ██████████████░░░░░░  label`
fn row(label: &str, p: f64) {
    let p = p.clamp(0.0, 1.0);
    let filled = (p * 20.0).round() as usize;
    println!("  {:>5.1}%  {}{}  {label}", p * 100.0, "█".repeat(filled), "░".repeat(20 - filled));
}
