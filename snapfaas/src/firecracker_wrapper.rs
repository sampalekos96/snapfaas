/// Wrapper for Firecracker vmm and vm
use std::sync::{Arc, RwLock, mpsc, mpsc::Sender, mpsc::channel};
use std::rc::Rc;
use std::thread::JoinHandle;
use std::io;

use futures::Future;
use futures::sync::oneshot;
use vmm::{VmmAction, VmmActionError, VmmData, VmmRequestOutcome};
use vmm::vmm_config::instance_info::{InstanceInfo, InstanceState};
use vmm::vmm_config::boot_source::BootSourceConfig;
use vmm::vmm_config::drive::BlockDeviceConfig;
use vmm::vmm_config::net::NetworkInterfaceConfig;
use vmm::vmm_config::vsock::VsockDeviceConfig;
use vmm::vmm_config::machine_config::VmConfig;
use vmm::SnapFaaSConfig;
use sys_util::EventFd;

pub struct VmmWrapper {
    vmm_thread_handle: JoinHandle<()>,
    vmm_action_sender: Sender<Box<VmmAction>>,
    event_fd: Rc<EventFd>,
}

#[derive(Debug)]
pub enum VmmError {
    EventFd(io::Error),
    ActionError(VmmActionError),
    ActionSender(mpsc::SendError<Box<VmmAction>>),
    SyncChannel(oneshot::Canceled),
}

impl VmmWrapper {
    pub fn new(id: String, config: SnapFaaSConfig) -> Result<VmmWrapper, VmmError> {
        let (vmm_action_sender, vmm_action_receiver) = channel();

        let shared_info = Arc::new(RwLock::new(InstanceInfo {
                            state: InstanceState::Uninitialized,
                            id: id.clone(),
                            vmm_version: "0.1".to_string(),
        }));

        let event_fd = EventFd::new().map_err(|e| VmmError::EventFd(e))?;
        let event_fd = Rc::new(event_fd);
        let event_fd_clone = event_fd.try_clone().map_err(|e| VmmError::EventFd(e))?;

        let thread_handle =
            vmm::start_vmm_thread(shared_info.clone(),
                                  event_fd_clone,
                                  vmm_action_receiver,
                                  0, //seccomp::SECCOMP_LEVEL_NONE,
                                  config,
                                  );
        
        let vmm_wrapper = VmmWrapper {
            vmm_thread_handle: thread_handle,
            vmm_action_sender: vmm_action_sender,
            event_fd: event_fd,
        };

        return Ok(vmm_wrapper);
    }

    pub fn send_vmm_action(&mut self, action: VmmAction) -> Result<(), VmmError> {
        self.vmm_action_sender.send(Box::new(action)).map_err(|e| VmmError::ActionSender(e))?;
        self.event_fd.write(1).map_err(|e| VmmError::EventFd(e))?;
        return Ok(());
    }

    pub fn recv_vmm_action_ret(&self, receiver: oneshot::Receiver<VmmRequestOutcome>)
        -> Result<VmmData, VmmError> {
        let ret = receiver.wait().map_err(|e| VmmError::SyncChannel(e))?;
        return ret.map_err(|e| VmmError::ActionError(e));
    }

    pub fn request_vmm_action(&mut self,
                              action: VmmAction,
                              ret_receiver: oneshot::Receiver<VmmRequestOutcome>)
        -> Result<VmmData, VmmError> {

        println!("Action: {:?}", action);

        self.send_vmm_action(action)?;
        self.recv_vmm_action_ret(ret_receiver)
    }

    pub fn set_configuration(&mut self, machine_config: VmConfig) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::SetVmConfiguration(machine_config, sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn get_configuration(&mut self) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::GetVmConfiguration(sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn set_boot_source(&mut self, config: BootSourceConfig) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::ConfigureBootSource(config, sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn insert_block_device(&mut self, config: BlockDeviceConfig) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::InsertBlockDevice(config, sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn insert_network_device(&mut self, config: NetworkInterfaceConfig) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::InsertNetworkDevice(config, sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn add_vsock(&mut self, config: VsockDeviceConfig) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::InsertVsockDevice(config, sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }


    pub fn start_instance(&mut self) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::StartMicroVm(sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn shutdown_instance(&mut self) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::SendCtrlAltDel(sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn dump_working_set(&mut self) -> Result<VmmData, VmmError> {
        let (sync_sender, sync_receiver) = oneshot::channel();
        let action = VmmAction::DumpWorkingSet(sync_sender);
        self.request_vmm_action(action, sync_receiver)
    }

    pub fn join_vmm(self) {
        self.vmm_thread_handle.join().expect("Couldn't join on the VMM thread");
    }
}
