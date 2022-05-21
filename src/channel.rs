use std::borrow::BorrowMut;
use std::ops::{Deref, DerefMut};
use constant::{ssh_msg_code, size, ssh_str};
use error::{SshError, SshErrorKind, SshResult};
use packet::Data;
use slog::log;
use crate::channel_exec::ChannelExec;
// use crate::channel_scp::ChannelScp;
use crate::channel_shell::ChannelShell;
use crate::kex::{Kex, processing_server_algorithm};
use crate::{Client, client, util};
use crate::window_size::WindowSize;

pub struct Channel {
    pub(crate) kex: Kex,
    pub(crate) remote_close: bool,
    pub(crate) local_close: bool,
    pub(crate) window_size: WindowSize
}

impl Deref for Channel {
    type Target = WindowSize;

    fn deref(&self) -> &Self::Target {
        &self.window_size
    }
}

impl DerefMut for Channel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.window_size
    }
}

impl Channel {
    pub(crate) fn other(&mut self, message_code: u8, mut result: Data) -> SshResult<()> {
        match message_code {
            ssh_msg_code::SSH_MSG_GLOBAL_REQUEST => {
                let mut data = Data::new();
                data.put_u8(ssh_msg_code::SSH_MSG_REQUEST_FAILURE);
                let mut client = client::locking()?;
                client.write(data)?;
                client::unlock(client)
            }
            ssh_msg_code::SSH_MSG_KEXINIT => {
                //let data = Packet::processing_data(result);
                let vec = result.to_vec();
                let mut data = Data::from(vec![message_code]);
                data.extend(vec);
                self.kex.h.set_i_s(data.as_slice());
                processing_server_algorithm(data)?;
                self.kex.send_algorithm()?;
                let config = util::config()?;

                let (dh, sign) = config.algorithm.matching_algorithm()?;
                self.kex.dh = dh;
                self.kex.signature = sign;

                self.kex.h.set_v_c(config.version.client_version.as_str());
                self.kex.h.set_v_s(config.version.server_version.as_str());

                util::unlock(config);

                self.kex.send_qc()?;
            }
            ssh_msg_code::SSH_MSG_KEX_ECDH_REPLY => {
                // 生成session_id并且获取signature
                let sig = self
                    .kex
                    .generate_session_id_and_get_signature(result)?;
                // 验签
                let r = self
                    .kex
                    .signature
                    .verify_signature(&self.kex.h.k_s, &self.kex.session_id, &sig)?;
                log::info!("signature Verification Result => {}", r);
                if !r {
                    return Err(SshError::from(SshErrorKind::SignatureError))
                }
            }
            ssh_msg_code::SSH_MSG_NEWKEYS => self.kex.new_keys()?,
            // 通道大小 暂不处理
            ssh_msg_code::SSH_MSG_CHANNEL_WINDOW_ADJUST => {
                // 接收方通道号， 暂时不需要
                result.get_u32();
                // 需要调整增加的窗口大小
                let rws = result.get_u32();
                self.window_size.add_remote_window_size(rws);
            },
            ssh_msg_code::SSH_MSG_CHANNEL_EOF => {}
            ssh_msg_code::SSH_MSG_CHANNEL_REQUEST => {}
            ssh_msg_code::SSH_MSG_CHANNEL_SUCCESS => {}
            ssh_msg_code::SSH_MSG_CHANNEL_FAILURE => return Err(SshError::from(SshErrorKind::ChannelFailureError)),
            ssh_msg_code::SSH_MSG_CHANNEL_CLOSE => {
                let cc = result.get_u32();
                if cc == self.client_channel {
                    self.remote_close = true;
                    self.close()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn open_shell(self) -> SshResult<ChannelShell> {
        log::info!("shell opened.");
        return ChannelShell::open(self)
    }

    pub fn open_exec(self) -> SshResult<ChannelExec> {
        log::info!("exec opened.");
        return Ok(ChannelExec::open(self))
    }

    // pub fn open_scp(mut self) -> SshResult<ChannelScp> {
    //     log::info!("scp opened.");
    //     return Ok(ChannelScp::open(self))
    // }

    pub fn close(&mut self) -> SshResult<()> {
        log::info!("channel close.");
        self.send_close()?;
        self.receive_close()
    }

    fn send_close(&mut self) -> SshResult<()> {
        if self.local_close { return Ok(()); }
        let mut data = Data::new();
        data.put_u8(ssh_msg_code::SSH_MSG_CHANNEL_CLOSE)
            .put_u32(self.server_channel);
        let mut client = client::locking()?;
        client.write(data)?;
        self.local_close = true;
        Ok(())
    }

    fn receive_close(&mut self) -> SshResult<()> {
        if self.remote_close { return Ok(()); }
        loop {
            let mut client = client::locking()?;
            let results = client.read()?; // close 时不消耗窗口空间
            client::unlock(client);
            for mut result in results {
                if result.is_empty() { continue }
                let message_code = result.get_u8();
                match message_code {
                    ssh_msg_code::SSH_MSG_CHANNEL_CLOSE => {
                        let cc = result.get_u32();
                        if cc == self.client_channel {
                            self.remote_close = true;
                            return Ok(())
                        }
                    }
                    _ => self.other(message_code, result)?
                }
            }
        }
    }

}


// pub(crate) struct ChannelWindowSize {
//     pub(crate) client_channel: u32,
//     pub(crate) server_channel: u32,
//     /// 本地窗口大小
//     pub(crate) window_size   : u32,
//     /// 远程窗口大小
//     pub(crate) r_window_size : u32
// }
//
// impl ChannelWindowSize {
//     pub(crate) fn new(client_channel: u32, server_channel: u32) -> ChannelWindowSize {
//         ChannelWindowSize{
//             client_channel,
//             server_channel,
//             window_size: 0,
//             r_window_size: 0
//         }
//     }
//     pub(crate) fn process_window_size(mut data: Data, client: &mut Client) -> SshResult<()> {
//
//         if data.is_empty() { return Ok(()) }
//
//         let msg_code = data.get_u8();
//
//         let (client_channel_no, size) = match msg_code {
//             ssh_msg_code::SSH_MSG_CHANNEL_DATA => {
//                 let client_channel_no = data.get_u32(); // channel serial no    4 len
//                 let vec = data.get_u8s(); // string data len
//                 let size = vec.len() as u32;
//                 (client_channel_no, size)
//             }
//             ssh_msg_code::SSH_MSG_CHANNEL_EXTENDED_DATA => {
//                 let client_channel_no = data.get_u32(); // channel serial no    4 len
//                 data.get_u32(); // data type code        4 len
//                 let vec = data.get_u8s();  // string data len
//                 let size = vec.len() as u32;
//                 (client_channel_no, size)
//             }
//             _ => return Ok(())
//         };
//
//         if size <= 0 { return Ok(()) }
//
//         if let Some(mut map) = util::get_channel_window(client_channel_no)?
//         {
//
//             *map += size;
//
//             if map.window_size >= (size::LOCAL_WINDOW_SIZE / 2) {
//                 let mut data = Data::new();
//                 data.put_u8(ssh_msg_code::SSH_MSG_CHANNEL_WINDOW_ADJUST)
//                     .put_u32(map.server_channel)
//                     .put_u32(size::LOCAL_WINDOW_SIZE - map.window_size);
//                 client.write(data)?;
//                 map.window_size = 0;
//             }
//         }
//
//         Ok(())
//     }
//
//     pub(crate) fn add_remote_window_size(&mut self, rws: u32) {
//         self.r_window_size = self.r_window_size + rws;
//     }
// }
//
// impl std::ops::AddAssign<u32> for ChannelWindowSize {
//     fn add_assign(&mut self, rhs: u32) {
//         self.window_size += rhs;
//     }
// }
