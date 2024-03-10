// Standard Uses

// Crate Uses
use crate::setups::jrpc_tcp::generated::{
    schemas::provider::GreetProviderProtocol,
    provider::GreetProvider
};

// External Uses
use comline_runtime::setup::CallResult;
use comline_runtime::setup::{
    transport::{methods::tcp::provider::TcpProvider, provider::ProviderSetup},
    call_system::systems::json_rpc::JsonRPCv2,
};


impl<CS> GreetProviderProtocol for GreetProvider<CS> {
    fn greet(&self, name: &str) -> CallResult<String> {
        println!("[Server] Received a greet request with name '{}'", name);

        Ok("Hello ".to_owned() + name)
    }
}


pub(crate) async fn main() {
    println!("Running Server");

    let (address, port) = ("127.0.0.1", "2620");
    let full_address = &*(address.to_owned() + ":" + port);

    let transporter = TcpProvider::with_address(full_address).await.unwrap();
    let mut setup = ProviderSetup::with_transporter(transporter)
        .with_call_system::<JsonRPCv2, _>(JsonRPCv2::new)
        .with_capability(GreetProvider::new)
    ;

    respond_to_incoming_hellos(&mut setup).await;
}

async fn respond_to_incoming_hellos(setup: &mut ProviderSetup<TcpProvider, JsonRPCv2>) {
    setup.transporter.read().unwrap()
        .listen_incoming_connection()
        .await;
}

