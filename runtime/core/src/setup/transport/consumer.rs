// Standard Uses
use std::sync::{Arc, RwLock};

// Crate Uses
use crate::setup::call_system::consumer::CallSystemConsumer;
use crate::setup::call_system::Origin;

// External Uses
use async_trait::async_trait;
use downcast_rs::{DowncastSync, impl_downcast};


#[async_trait]
pub trait CommunicationConsumer: Send + Sync {
    async fn connect_to_provider(&self);

    fn send_data(&mut self, data: &[u8]) -> eyre::Result<()>;
    async fn send_data_async(&mut self, data: &[u8]) -> eyre::Result<()>;
}

pub struct ConsumerSetup<T, CC> where T: CommunicationConsumer, CC: CallSystemConsumer {
    pub transport_method: Arc<RwLock<T>>,
    pub call_system: Option<Arc<RwLock<CC>>>,
    pub capabilities: Vec<Box<dyn ConsumerCapability>>
}

impl<T, CC> ConsumerSetup<T, CC> where T: CommunicationConsumer + 'static, CC: CallSystemConsumer {
    pub fn with_transport(transporter: T) -> Self {
        Self {
            transport_method: Arc::new(RwLock::new(transporter)),
            call_system: None,
            capabilities: vec![],
        }
    }

    pub fn with_call_system<CFn>(mut self, call_system: CFn) -> Self
        where CFn: FnOnce(Origin) -> CC
    {
        self.call_system = Some(Arc::new(RwLock::new(call_system(
            Origin::Consumer(self.transport_method.clone()),
        ))));
        self
    }

    pub fn with_capability<C, Cfn>(mut self, capability: Cfn) -> Self
        where
            C: ConsumerCapability,
            Cfn: FnOnce(Arc<RwLock<CC>>) -> C
    {
        self.capabilities.push(Box::new(capability(self.call_system.as_ref().unwrap().clone())));
        self
    }

    pub fn into_threaded(self) -> Arc<RwLock<Self>> { Arc::new(RwLock::new(self)) }

    pub fn add_default_capability<C, Cfn>(mut self, capability: Cfn) -> Self
        where
            C: ConsumerCapability,
            Cfn: FnOnce(Arc<RwLock<CC>>) -> C
    {
        self.capabilities.push(Box::new(capability(self.call_system.as_ref().unwrap().clone())));
        self
    }

    // TODO: Unsure if these are necessary right now, their signatures are also incorrect
    //       they should have the same parameters ad the `ẁith_capability` method
    /*
    pub fn add_capability<
        C: ConsumerCapability,
        Cfn: Fn(&ConsumerSetup) -> C
    >(
        &mut self, capability_fn: Cfn
    ) {
        self.capabilities.push(Box::new(capability_fn(&*self)));
    }

    pub fn add_capabilities(&mut self, mut capabilities: Vec<Box<dyn ConsumerCapability>>) {
        self.capabilities.append(&mut capabilities);
    }
    */

    pub fn capability<C: ConsumerCapability>(&self) -> Option<&C> {
        for capability in self.capabilities.iter() {
            if let Some(cap) = capability.downcast_ref::<C>() {
                return Some(cap);
            }
        }

        None
    }

    pub fn capability_mut<C: ConsumerCapability>(&mut self) -> Option<&mut C> {
        for capability in &mut self.capabilities {
            if let Some(cap) = capability.downcast_mut::<C>() {
                return Some(&mut *cap);
            }
        }

        None
    }
}


pub type SharedConsumerSetup<T, CC> = Arc<RwLock<ConsumerSetup<T, CC>>>;

pub trait ConsumerCapability: DowncastSync {
    //fn setup(&self) -> Arc<RwLock<ConsumerSetup>>;
}
impl_downcast!(sync ConsumerCapability);

