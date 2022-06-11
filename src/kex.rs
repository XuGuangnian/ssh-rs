use std::sync::atomic::Ordering;
use crate::constant::ssh_msg_code;
use crate::encryption::{ChaCha20Poly1305, H, PublicKey, SIGN, RSA, HASH, digest, IS_ENCRYPT, AesCtr};
use crate::error::{SshError, SshErrorKind, SshResult};
use crate::data::Data;
use crate::slog::log;
use crate::config::{
    CompressionAlgorithm,
    EncryptionAlgorithm,
    KeyExchangeAlgorithm,
    MacAlgorithm,
    PublicKeyAlgorithm
};
use crate::{client, config, encryption, util};
use crate::algorithm::{hash, key_exchange};


pub(crate) struct Kex {
    pub(crate) session_id: Vec<u8>,
    pub(crate) h: H,
    pub(crate) signature: Box<SIGN>
}

impl Kex {

    pub(crate) fn new() -> SshResult<Kex> {
        Ok(Kex {
            session_id: vec![],
            h: H::new(),
            signature: Box::new(RSA::new())
        })
    }


    pub(crate) fn send_algorithm(&mut self) -> SshResult<()> {
        let config = config::config();
        log::info!("client algorithms: [{}]", config.algorithm.client_algorithm.to_string());
        if IS_ENCRYPT.load(Ordering::Relaxed) {
            IS_ENCRYPT.store(false, Ordering::Relaxed);
            encryption::update_encryption_key(None);
        }
        let mut data = Data::new();
        data.put_u8(ssh_msg_code::SSH_MSG_KEXINIT);
        data.extend(util::cookie());
        data.extend(config.algorithm.client_algorithm.as_i());
        data.put_str("")
            .put_str("")
            .put_u8(false as u8)
            .put_u32(0_u32);

        self.h.set_i_c(data.as_slice());

        let client = client::default()?;
        client.write(data)
    }


    pub(crate) fn receive_algorithm(&mut self) -> SshResult<()> {
        let client = client::default()?;
        loop {
            let results = client.read()?;
            for result in results {
                if result.is_empty() { continue }
                let message_code = result[0];
                match message_code {
                    ssh_msg_code::SSH_MSG_KEXINIT => {
                        self.h.set_i_s(result.as_slice());
                        return processing_server_algorithm(result)
                    }
                    _ => { }
                }
            }
        }
    }


    pub(crate) fn send_qc(&self) -> SshResult<()> {
        let mut data = Data::new();
        data.put_u8(ssh_msg_code::SSH_MSG_KEX_ECDH_INIT);
        data.put_u8s(key_exchange::get().get_public_key());
        let client = client::default()?;
        client.write(data)
    }


    pub(crate) fn verify_signature_and_new_keys(&mut self) -> SshResult<()> {
        loop {
            let client = client::default()?;
            let results = client.read()?;
            for mut result in results {
                if result.is_empty() { continue }
                let message_code = result.get_u8();
                match message_code {
                    ssh_msg_code::SSH_MSG_KEX_ECDH_REPLY => {
                        // 生成session_id并且获取signature
                        let sig = self.generate_session_id_and_get_signature(result)?;
                        // 验签
                        let r = self
                            .signature
                            .verify_signature(&self.h.k_s, &self.session_id, &sig)?;
                        log::info!("signature verification result: [{}]", r);
                        if !r {
                            return Err(SshError::from(SshErrorKind::SignatureError))
                        }
                    }
                    ssh_msg_code::SSH_MSG_NEWKEYS => {
                        self.new_keys()?;
                        log::info!("send new keys");
                        return Ok(())
                    }
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn new_keys(&mut self) -> Result<(), SshError> {
        let mut data = Data::new();
        data.put_u8(ssh_msg_code::SSH_MSG_NEWKEYS);
        let client = client::default()?;
        client.write(data)?;

        let hash: HASH = HASH::new(&self.h.k, &self.session_id, &self.session_id);
        // let poly1305 = ChaCha20Poly1305::new(hash);
        let ctr = AesCtr::new(hash);
        IS_ENCRYPT.store(true, Ordering::Relaxed);
        encryption::update_encryption_key(Some(ctr));
        Ok(())
    }

    pub(crate) fn generate_session_id_and_get_signature(&mut self, mut data: Data) -> Result<Vec<u8>, SshError> {
        let ks = data.get_u8s();
        self.h.set_k_s(&ks);
        // TODO 未进行密钥指纹验证！！
        let qs = data.get_u8s();
        self.h.set_q_c(key_exchange::get().get_public_key());
        self.h.set_q_s(&qs);
        let vec = key_exchange::get().get_shared_secret(qs)?;
        self.h.set_k(&vec);
        let hb = self.h.as_bytes();
        let hash_type = key_exchange::get().get_hash_type();
        self.session_id = hash::digest(hash_type, &hb).to_vec();
        let h = data.get_u8s();
        let mut hd = Data::from(h);
        hd.get_u8s();
        let signature = hd.get_u8s();
        Ok(signature)
    }
}

pub(crate) fn processing_server_algorithm(mut data: Data) -> SshResult<()> {
    data.get_u8();
    // 跳过16位cookie
    data.skip(16);
    let config = config::config();
    let server_algorithm = &mut config.algorithm.server_algorithm;
    server_algorithm.key_exchange_algorithm     =   KeyExchangeAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.public_key_algorithm       =   PublicKeyAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.c_encryption_algorithm     =   EncryptionAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.s_encryption_algorithm     =   EncryptionAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.c_mac_algorithm            =   MacAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.s_mac_algorithm            =   MacAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.c_compression_algorithm    =   CompressionAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    server_algorithm.s_compression_algorithm    =   CompressionAlgorithm(util::vec_u8_to_string(data.get_u8s(), ",")?);
    log::info!("server algorithms: [{}]", server_algorithm.to_string());
    return Ok(())
}
