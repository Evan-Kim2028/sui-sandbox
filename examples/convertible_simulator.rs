use std::env;

#[derive(Debug)]
struct Args {
    principal: f64,
    yield_bps: f64,
    strike: f64,
    end: f64,
    apy: f64,
    years: f64,
}

impl Args {
    fn parse() -> Self {
        let mut args = env::args().skip(1);
        let mut out = Args {
            principal: 1000.0,
            yield_bps: 500.0,
            strike: 2000.0,
            end: 2000.0,
            apy: 0.05,
            years: 1.0,
        };

        while let Some(arg) = args.next() {
            let val = args.next().unwrap_or_default();
            match arg.as_str() {
                "--principal" => out.principal = val.parse().unwrap_or(out.principal),
                "--yield-bps" => out.yield_bps = val.parse().unwrap_or(out.yield_bps),
                "--strike" => out.strike = val.parse().unwrap_or(out.strike),
                "--end" => out.end = val.parse().unwrap_or(out.end),
                "--apy" => out.apy = val.parse().unwrap_or(out.apy),
                "--years" => out.years = val.parse().unwrap_or(out.years),
                _ => {}
            }
        }

        out
    }
}

fn main() {
    let args = Args::parse();

    let owed = args.principal * (1.0 + args.yield_bps / 10_000.0);
    let eth_multiplier = args.end / args.strike;

    let convertible = owed * eth_multiplier.max(1.0);
    let eth_hodl = args.principal * eth_multiplier;
    let stable = args.principal * (1.0 + args.apy).powf(args.years);

    println!("Convertible note simulator");
    println!("---------------------------");
    println!("principal:   ${:.2}", args.principal);
    println!("yield_bps:   {:.2}", args.yield_bps);
    println!("strike:      ${:.2} per ETH", args.strike);
    println!("end price:   ${:.2} per ETH", args.end);
    println!("apy:         {:.2}%", args.apy * 100.0);
    println!("years:       {:.2}", args.years);
    println!();

    println!("Outcome (USD)");
    println!("- convertible: ${:.2}", convertible);
    println!("- ETH hodl:    ${:.2}", eth_hodl);
    println!("- stable APY:  ${:.2}", stable);
}
