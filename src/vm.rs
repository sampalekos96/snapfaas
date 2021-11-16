#[cfg(not(test))]
use log::{error, info};
#[cfg(not(test))]
use std::net::Shutdown;
#[cfg(not(test))]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
#[cfg(not(test))]
use std::process::{Child, Command, Stdio};
use std::string::String;
#[cfg(not(test))]
use std::time::Instant;

#[cfg(not(test))]
use crate::configs::FunctionConfig;
#[cfg(not(test))]
use crate::request::Request;
use crate::syscalls;

use labeled::dclabel::{Clause, Component, DCLabel};
use labeled::Label;

#[derive(Debug)]
pub enum Error {
    ProcessSpawn(std::io::Error),
    //VmTimeout(RecvTimeoutError),
    VmWrite(std::io::Error),
    VmRead(std::io::Error),
    SyscallError(std::io::Error),
    KernelNotExist,
    RootfsNotExist,
    AppfsNotExist,
    LoadDirNotExist,
    NotString(std::string::FromUtf8Error),
}

pub struct OdirectOption {
    pub base: bool,
    pub diff: bool,
    pub rootfs: bool,
    pub appfs: bool,
}

#[derive(Debug)]
pub struct VmAppConfig {
    pub rootfs: String,
    pub appfs: String,
    pub load_dir: Vec<PathBuf>,
    pub dump_dir: Option<PathBuf>,
}

#[derive(Debug)]
#[cfg(not(test))]
pub struct Vm {
    pub id: usize,
    pub memory: usize, // MB
    pub function_name: String,
    //vsock_stream: VsockStream,
    conn: UnixStream,
    process: Child,
    current_label: DCLabel,
    /*
    pub process: Pid,
    cgroup_name: PathBuf,
    pub cpu_share: usize,
    pub vcpu_count: usize,
    pub kernel: String,
    pub kernel_args: String,
    pub ready_notifier: File, // Vm writes to this File when setup finishes

    pub app_config: VmAppConfig,
    */
}

#[cfg(not(test))]
impl Vm {
    /// Launch a vm instance and return a Vm value
    /// When this function returns, the VM has finished booting and is ready
    /// to accept requests.
    pub fn new(
        id: &str,
        function_config: &FunctionConfig,
        vm_listener: &UnixListener,
        cid: u32,
        network: Option<&str>,
        firerunner: &str,
        force_exit: bool,
        odirect: Option<OdirectOption>,
    ) -> Result<(Vm, Vec<Instant>), Error> {
        let mut ts_vec = Vec::with_capacity(10);
        ts_vec.push(Instant::now());
        let mut cmd = Command::new(firerunner);
        let mem_str = function_config.memory.to_string();
        let vcpu_str = function_config.vcpus.to_string();
        let cid_str = cid.to_string();
        let mut args = vec![
            "--id",
            id,
            "--kernel",
            &function_config.kernel,
            "--mem_size",
            &mem_str,
            "--vcpu_count",
            &vcpu_str,
            "--rootfs",
            &function_config.runtimefs,
            "--cid",
            &cid_str,
        ];

        if function_config.appfs != "" {
            args.extend_from_slice(&["--appfs", &function_config.appfs]);
        }
        if let Some(load_dir) = function_config.load_dir.as_ref() {
            args.extend_from_slice(&["--load_from", &load_dir]);
        }
        if let Some(dump_dir) = function_config.dump_dir.as_ref() {
            args.extend_from_slice(&["--dump_to", &dump_dir]);
        }
        //if let Some(diff_dirs) = function_config.diff_dirs.as_ref() {
        //    args.extend_from_slice(&["--diff_dirs", &diff_dirs]);
        //}
        if let Some(cmdline) = function_config.cmdline.as_ref() {
            args.extend_from_slice(&["--kernel_args", &cmdline]);
        }
        if function_config.dump_ws {
            args.push("--dump_ws");
        }
        if function_config.load_ws {
            args.push("--load_ws");
        }
        if function_config.copy_base {
            args.push("--copy_base");
        }
        if function_config.copy_diff {
            args.push("--copy_diff");
        }

        // network config should be of the format <TAP-Name>/<MAC Address>
        if let Some(network) = network {
            let v: Vec<&str> = network.split('/').collect();
            args.extend_from_slice(&["--tap_name", v[0]]);
            args.extend_from_slice(&["--mac", v[1]]);
        }

        // odirect
        if let Some(odirect) = odirect {
            if odirect.base {
                args.push("--odirect_base");
            }
            if !odirect.diff {
                args.push("--no_odirect_diff");
            }
            if !odirect.rootfs {
                args.push("--no_odirect_root");
            }
            if !odirect.appfs {
                args.push("--no_odirect_app");
            }
        }

        info!("args: {:?}", args);
        let cmd = cmd.args(args);
        let vm_process: Child = cmd
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::ProcessSpawn(e))?;
        ts_vec.push(Instant::now());

        if force_exit {
            let output = vm_process
                .wait_with_output()
                .expect("failed to wait on child");
            let mut status = 0;
            if !output.status.success() {
                eprintln!("{:?}", String::from_utf8_lossy(&output.stderr));
                status = 1;
            }
            crate::unlink_unix_sockets();
            std::process::exit(status);
        }

        let (conn, _) = vm_listener.accept().unwrap();
        ts_vec.push(Instant::now());

        return Ok((
            Vm {
                id: id.parse::<usize>().expect("vm id not int"),
                function_name: function_config.name.clone(),
                memory: function_config.memory,
                conn,
                process: vm_process,
                // We should also probably have a clearance to mitigate side channel attacks, but
                // meh for now...
                current_label: DCLabel::public(),
            },
            ts_vec,
        ));
    }

    /// Send request to vm and wait for its response
    pub fn process_req(&mut self, req: Request) -> Result<String, Error> {
        use prost::Message;
        use std::io::Write;

        let sys_req = syscalls::Request {
            payload: req.payload_as_string(),
        }
        .encode_to_vec();

        self.conn
            .write_all(&(sys_req.len() as u32).to_be_bytes())
            .map_err(|e| Error::VmWrite(e))?;
        self.conn
            .write_all(sys_req.as_ref())
            .map_err(|e| Error::VmWrite(e))?;

        self.process_syscall().map_err(|e| Error::SyscallError(e))
    }

    pub fn process_syscall(&mut self) -> Result<String, std::io::Error> {
        use lmdb::{Transaction, WriteFlags};
        use prost::Message;
        use std::io::{Read, Write};
        use syscalls::syscall::Syscall as SC;
        use syscalls::Syscall;

        let dbenv = lmdb::Environment::new()
            .open(std::path::Path::new("storage"))
            .unwrap();
        let default_db = dbenv.open_db(None).unwrap();

        loop {
            let buf = {
                let mut lenbuf = [0; 4];
                self.conn.read_exact(&mut lenbuf)?;
                let size = u32::from_be_bytes(lenbuf);
                let mut buf = vec![0u8; size as usize];
                self.conn.read_exact(&mut buf)?;
                buf
            };
            match Syscall::decode(buf.as_ref())?.syscall {
                Some(SC::Response(r)) => {
                    return Ok(r.payload);
                }
                Some(SC::ReadKey(rk)) => {
                    let txn = dbenv.begin_ro_txn().unwrap();
                    let result = syscalls::ReadKeyResponse {
                        value: txn.get(default_db, &rk.key).ok().map(Vec::from),
                    };
                    let _ = txn.commit();
                    self.conn
                        .write_all(&(result.encoded_len() as u32).to_be_bytes())?;
                    self.conn.write_all(result.encode_to_vec().as_ref())?;
                }
                Some(SC::WriteKey(wk)) => {
                    let mut txn = dbenv.begin_rw_txn().unwrap();
                    let result = syscalls::WriteKeyResponse {
                        success: txn
                            .put(default_db, &wk.key, &wk.value, WriteFlags::empty())
                            .is_ok(),
                    };
                    let _ = txn.commit();
                    self.conn
                        .write_all(&(result.encoded_len() as u32).to_be_bytes())?;
                    self.conn.write_all(result.encode_to_vec().as_ref())?;
                }
                Some(SC::GetCurrentLabel(_)) => {
                    let result = syscalls::DcLabel {
                        secrecy: match &self.current_label.secrecy {
                            Component::DCFalse => None,
                            Component::DCFormula(set) => Some(syscalls::Component {
                                clauses: set
                                    .iter()
                                    .map(|clause| syscalls::Clause {
                                        principals: clause.0.iter().map(Clone::clone).collect(),
                                    })
                                    .collect(),
                            }),
                        },
                        integrity: match &self.current_label.integrity {
                            Component::DCFalse => None,
                            Component::DCFormula(set) => Some(syscalls::Component {
                                clauses: set
                                    .iter()
                                    .map(|clause| syscalls::Clause {
                                        principals: clause.0.iter().map(Clone::clone).collect(),
                                    })
                                    .collect(),
                            }),
                        },
                    };
                    self.conn
                        .write_all(&(result.encoded_len() as u32).to_be_bytes())?;
                    self.conn.write_all(result.encode_to_vec().as_ref())?;
                }
                Some(SC::TaintWithLabel(label)) => {
                    let dclabel = DCLabel {
                        secrecy: match label.secrecy {
                            None => Component::DCFalse,
                            Some(set) => Component::DCFormula(
                                set.clauses
                                    .iter()
                                    .map(|c| {
                                        Clause(c.principals.iter().map(Clone::clone).collect())
                                    })
                                    .collect(),
                            ),
                        },
                        integrity: match label.integrity {
                            None => Component::DCFalse,
                            Some(set) => Component::DCFormula(
                                set.clauses
                                    .iter()
                                    .map(|c| {
                                        Clause(c.principals.iter().map(Clone::clone).collect())
                                    })
                                    .collect(),
                            ),
                        },
                    };
                    self.current_label = self.current_label.clone().lub(dclabel);
                    let result = syscalls::DcLabel {
                        secrecy: match &self.current_label.secrecy {
                            Component::DCFalse => None,
                            Component::DCFormula(set) => Some(syscalls::Component {
                                clauses: set
                                    .iter()
                                    .map(|clause| syscalls::Clause {
                                        principals: clause.0.iter().map(Clone::clone).collect(),
                                    })
                                    .collect(),
                            }),
                        },
                        integrity: match &self.current_label.integrity {
                            Component::DCFalse => None,
                            Component::DCFormula(set) => Some(syscalls::Component {
                                clauses: set
                                    .iter()
                                    .map(|clause| syscalls::Clause {
                                        principals: clause.0.iter().map(Clone::clone).collect(),
                                    })
                                    .collect(),
                            }),
                        },
                    };
                    self.conn
                        .write_all(&(result.encoded_len() as u32).to_be_bytes())?;
                    self.conn.write_all(result.encode_to_vec().as_ref())?;
                }
                None => {
                    // Should never happen, so just ignore??
                }
            }
        }
    }

    /// shutdown this vm
    pub fn shutdown(&mut self) {
        // TODO: not sure if kill() waits for the child process to terminate
        // before returning. This is relevant for shutdown latency measurement.
        // TODO: std::process::Child.kill() is equivalent to sending a SIGKILL
        // on unix platforms which means the child process won't be able to run
        // its clean up process. Previously, we shutdown VMs through SIGTERM
        // which does allow a shutdown process. We need to make sure using
        // SIGKILL won't create any issues with vms.
        if let Err(e) = self.conn.shutdown(Shutdown::Both) {
            error!("Failed to shut down unix connection: {:?}", e);
        }
        if let Err(e) = self.process.kill() {
            error!("VM already exited: {:?}", e);
        }
    }
}

#[derive(Debug)]
#[cfg(test)]
pub struct Vm {
    pub id: usize,
    pub memory: usize, // MB
}

#[cfg(test)]
impl Vm {
    /// Create a dummy VM for controller tests
    pub fn new_dummy(memory: usize) -> Self {
        Vm { id: 0, memory }
    }

    pub fn process_req(&mut self, _: Request) -> Result<String, Error> {
        Ok(String::new())
    }

    pub fn shutdown(&mut self) {}
}
