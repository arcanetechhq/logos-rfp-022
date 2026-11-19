use lee_core::program::{ReceiveInput, Response};
use redemption_inbound::{Message, Policy, handle};

lee_core::actor_entrypoint!(dispatch);

fn dispatch(input: &ReceiveInput, message: Message) -> Response {
    let policy: Policy = serde_json::from_str(include_str!(env!("RFP022_INBOUND_POLICY")))
        .expect("valid policy embedded into the mint program");
    handle(input, &policy, message).unwrap_or_else(|error| panic!("{error}"))
}
