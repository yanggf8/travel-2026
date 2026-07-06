pub struct ContentDepthArgs {
    pub plan_id: String,
    pub against: String,
}

impl ContentDepthArgs {
    pub fn parse(rest: &[String]) -> Result<Self, String> {
        let mut plan_id: Option<String> = None;
        let mut against = "okinawa-2026".to_string();
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--plan-id" => {
                    plan_id = Some(
                        rest.get(i + 1)
                            .ok_or("--plan-id needs a value")?
                            .clone(),
                    );
                    i += 2;
                }
                "--against" => {
                    against = rest
                        .get(i + 1)
                        .ok_or("--against needs a value")?
                        .clone();
                    i += 2;
                }
                other => {
                    return Err(format!("unknown flag for compare content-depth: {other}"));
                }
            }
        }
        Ok(Self {
            plan_id: plan_id.ok_or("compare content-depth requires --plan-id <drill>")?,
            against,
        })
    }
}

fn destination_for(plan_id: &str) -> String {
    plan_id.replace('-', "_")
}

pub async fn run(rest: &[String]) -> Result<(), String> {
    let args = ContentDepthArgs::parse(rest)?;
    let _ = (
        &args.plan_id,
        &args.against,
        destination_for(&args.plan_id),
    );
    Ok(())
}