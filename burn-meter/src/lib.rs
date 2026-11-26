//! User-cycle metering for the pinned native RISC Zero actor and kernel.
//! Instruction execution is the upstream 4.1.0 RV32IM decoder. Paging and
//! segment padding are outside the LEZ user-cycle count.
use anyhow::{Result, bail, ensure};
use risc0_binfmt::{ByteAddr, ProgramBinary, WordAddr};
use risc0_zkvm::sha::{Digest, Impl, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[allow(dead_code)]
mod rv32im;
use rv32im::{DecodedInstruction, EmuContext, Emulator, Exception, InsnKind};

mod platform {
    pub const REG_MAX: usize = 32;
    pub const REG_ZERO: usize = 0;
    pub const WORD_SIZE: usize = 4;
}

const LOW: u32 = 0x10000;
const KERNEL: u32 = 0xc0000000;
const MACHINE_REGS: u32 = 0xffff0000;
const USER_REGS: u32 = 0xffff0080;
const SAFE_WRITE: u32 = 0xffff0100;
const MEPC: u32 = 0xffff0200;
const SUSPEND_PC: u32 = 0xffff0210;
const SUSPEND_MODE: u32 = 0xffff0214;
const ECALL_DISPATCH: u32 = 0xffff1000;
const PAGE_BYTES: u32 = 1024;

#[derive(Clone)]
pub struct Image(BTreeMap<u32, Box<[u32; 256]>>);

impl Image {
    /// Project the same ELF and kernel memory that the RISC Zero executor loads.
    pub fn from_binary(blob: &[u8]) -> Result<Self> {
        let native = ProgramBinary::decode(blob)?.to_image()?;
        Ok(Self(
            native
                .into_pages()
                .into_iter()
                .map(|(index, page)| {
                    let words = std::array::from_fn(|i| page.load(WordAddr(i as u32)));
                    (index, Box::new(words))
                })
                .collect(),
        ))
    }

    /// The image digest in the settlement guest authenticates this encoding.
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(self.0.len() * (4 + PAGE_BYTES as usize));
        for (&index, page) in &self.0 {
            encoded.extend(index.to_le_bytes());
            for word in page.as_ref() {
                encoded.extend(word.to_le_bytes());
            }
        }
        encoded
    }

    pub fn decode(encoded: &[u8]) -> Result<Self> {
        let chunks = encoded.chunks_exact(4 + PAGE_BYTES as usize);
        ensure!(chunks.remainder().is_empty(), "invalid memory image length");
        let mut pages = BTreeMap::new();
        for chunk in chunks {
            let index = u32::from_le_bytes(chunk[..4].try_into()?);
            ensure!(index <= u32::MAX / PAGE_BYTES, "invalid page index");
            ensure!(
                pages.last_key_value().is_none_or(|(last, _)| index > *last),
                "unordered pages"
            );
            let words = std::array::from_fn(|i| {
                let offset = 4 + i * 4;
                u32::from_le_bytes(chunk[offset..offset + 4].try_into().unwrap())
            });
            pages.insert(index, Box::new(words));
        }
        Ok(Self(pages))
    }

    pub fn execute(&self, input: &[u8], limit: u64) -> Result<Outcome> {
        let memory = Memory::from_image(self);
        let mut machine = Machine {
            memory,
            registers: std::array::from_fn(|i| self.word(MACHINE_REGS + i as u32 * 4)),
            mode: self.word(SUSPEND_MODE),
            pc: ByteAddr(self.word(SUSPEND_PC)),
            cycles: 0,
            input,
            journal: Vec::new(),
            syscall_return: [0; 8],
            exit: None,
            random_words: BTreeSet::new(),
        };
        let mut decoder = Emulator::new();
        while machine.exit.is_none() {
            if machine.cycles >= limit {
                return Ok(Outcome::OutOfGas);
            }
            decoder.step(&mut machine)?;
        }
        match machine.exit.unwrap() {
            Exit::Halted => {
                ensure!(machine.input.is_empty(), "unused actor input");
                Ok(Outcome::Halted {
                    cycles: machine.cycles,
                    journal: machine.journal,
                })
            }
            Exit::Panic => Ok(Outcome::Panic),
        }
    }

    #[inline]
    fn word(&self, address: u32) -> u32 {
        self.0
            .get(&(address / PAGE_BYTES))
            .map_or(0, |page| page[((address % PAGE_BYTES) / 4) as usize])
    }
}

type Page = [u32; 256];
type Bank = [Option<Box<Page>>; 256];

// Array-indexed two-level page table. The pinned actor uses a few small
// address ranges so the inner machine need not materialize a 4 GiB buffer.
struct Memory {
    banks: Vec<Option<Box<Bank>>>,
}

impl Memory {
    fn from_image(image: &Image) -> Self {
        let mut memory = Self {
            banks: vec![None; 1 << 14],
        };
        for (&index, page) in &image.0 {
            let bank = memory.bank(index);
            bank[(index & 255) as usize] = Some(page.clone());
        }
        memory
    }

    #[inline(always)]
    fn bank(&mut self, index: u32) -> &mut Bank {
        self.banks[(index >> 8) as usize]
            .get_or_insert_with(|| Box::new(std::array::from_fn(|_| None)))
    }

    #[inline(always)]
    fn word(&self, addr: u32) -> u32 {
        let Some(bank) = &self.banks[(addr >> 18) as usize] else {
            return 0;
        };
        let Some(page) = &bank[((addr >> 10) & 255) as usize] else {
            return 0;
        };
        page[((addr >> 2) & 255) as usize]
    }

    #[inline(always)]
    fn store(&mut self, addr: u32, word: u32) {
        let bank = self.bank(addr >> 10);
        let page = bank[((addr >> 10) & 255) as usize].get_or_insert_with(|| Box::new([0; 256]));
        page[((addr >> 2) & 255) as usize] = word;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Halted { cycles: u64, journal: Vec<u8> },
    OutOfGas,
    Panic,
}

enum Exit {
    Halted,
    Panic,
}

struct Machine<'a> {
    memory: Memory,
    registers: [u32; 64],
    mode: u32,
    pc: ByteAddr,
    cycles: u64,
    input: &'a [u8],
    journal: Vec<u8>,
    syscall_return: [u8; 8],
    exit: Option<Exit>,
    random_words: BTreeSet<u32>,
}

impl Machine<'_> {
    #[inline(always)]
    fn reg_index(&self, index: usize) -> usize {
        index + if self.mode == 0 { 32 } else { 0 }
    }

    #[inline(always)]
    fn read_word(&self, addr: u32) -> Result<u32> {
        let word = addr & !3;
        ensure!(
            !self.random_words.contains(&word),
            "read uninitialized random word {word:#x} at PC {:#x}",
            self.pc.0
        );
        let index = (word.wrapping_sub(MACHINE_REGS) >> 2) as usize;
        if index < self.registers.len() {
            return Ok(self.registers[index]);
        }
        Ok(self.memory.word(word))
    }

    #[inline(always)]
    fn write_word(&mut self, addr: u32, value: u32) {
        if !self.random_words.is_empty() {
            self.random_words.remove(&addr);
        }
        let index = (addr.wrapping_sub(MACHINE_REGS) >> 2) as usize;
        if index < self.registers.len() {
            self.registers[index] = value;
        } else {
            self.memory.store(addr, value);
        }
    }

    fn byte(&self, addr: u32) -> Result<u8> {
        Ok((self.read_word(addr)? >> (8 * (addr & 3))) as u8)
    }

    fn store_byte(&mut self, addr: u32, byte: u8) -> Result<()> {
        let shift = 8 * (addr & 3);
        let word = (self.read_word(addr)? & !(0xff << shift)) | (u32::from(byte) << shift);
        self.write_word(addr & !3, word);
        Ok(())
    }

    fn syscall_name(&self, addr: u32) -> Result<String> {
        let mut bytes = Vec::new();
        for offset in 0..128 {
            let byte = self.byte(addr.checked_add(offset).unwrap())?;
            if byte == 0 {
                return Ok(String::from_utf8(bytes)?);
            }
            bytes.push(byte);
        }
        bail!("invalid syscall name");
    }

    fn host_read(&mut self, name: u32, addr: u32, len: u32) -> Result<()> {
        ensure!(
            len <= 1024 && addr.checked_add(len).is_some(),
            "invalid host read"
        );
        let mut random = false;
        let data = match name {
            0 => {
                ensure!(len == 8, "invalid syscall return read");
                self.syscall_return.to_vec()
            }
            _ => {
                let syscall = self.syscall_name(name)?;
                let user_arg = |arg: u32| self.read_word(USER_REGS + 4 * arg);
                match syscall.as_str() {
                    "risc0_zkvm_platform::syscall::nr::SYS_READ" => {
                        let (fd, nbytes) = (user_arg(13)?, user_arg(14)?);
                        ensure!(fd == 0 && len <= nbytes, "unsupported actor stream read");
                        let (main, remaining) =
                            self.input.split_at((len as usize).min(self.input.len()));
                        let tail_len = (nbytes as usize - main.len()).min(remaining.len());
                        ensure!(tail_len <= 4, "unsupported unaligned read");
                        let (tail, remaining) = remaining.split_at(tail_len);
                        self.input = remaining;
                        let read = (main.len() + tail.len()) as u32;
                        self.syscall_return = [0; 8];
                        self.syscall_return[..4].copy_from_slice(&read.to_le_bytes());
                        self.syscall_return[4..4 + tail.len()].copy_from_slice(tail);
                        let mut data = main.to_vec();
                        data.resize(len as usize, 0);
                        data
                    }
                    "risc0_zkvm_platform::syscall::nr::SYS_WRITE" => {
                        let (fd, ptr, nbytes) = (user_arg(13)?, user_arg(14)?, user_arg(15)?);
                        ensure!(
                            len == 0 && (fd == 2 || fd == 3) && nbytes <= 8192,
                            "unsupported actor stream write"
                        );
                        let bytes: Vec<_> = (ptr..ptr.checked_add(nbytes).unwrap())
                            .map(|address| self.byte(address))
                            .collect::<Result<_>>()?;
                        if fd == 3 {
                            self.journal.extend_from_slice(&bytes);
                        }
                        self.syscall_return = [0; 8];
                        Vec::new()
                    }
                    "risc0_zkvm_platform::syscall::nr::SYS_RANDOM" => {
                        ensure!(
                            addr & 3 == 0 && len & 3 == 0 && addr + len <= MACHINE_REGS,
                            "unaligned randomness"
                        );
                        random = true;
                        self.syscall_return = [0; 8];
                        self.syscall_return[..4].copy_from_slice(&len.to_le_bytes());
                        vec![0; len as usize]
                    }
                    "risc0_zkvm_platform::syscall::nr::SYS_GETENV" => {
                        self.syscall_return = [0; 8];
                        self.syscall_return[..4].copy_from_slice(&u32::MAX.to_le_bytes());
                        vec![0; len as usize]
                    }
                    "risc0_zkvm_platform::syscall::nr::SYS_PANIC" => {
                        ensure!(len == 0, "invalid panic syscall");
                        self.exit = Some(Exit::Panic);
                        return Ok(());
                    }
                    _ => bail!("unsupported actor syscall {syscall}"),
                }
            }
        };
        // Machine entry, read setup, then one cycle per aligned group of
        // at most four words, or per unaligned prefix/suffix byte.
        self.cycles += 2;
        let mut bytes = data.as_slice();
        let mut ptr = addr;
        while !bytes.is_empty() {
            if ptr & 3 == 0 && bytes.len() >= 4 {
                let words = (bytes.len() / 4).min(4);
                for index in 0..4 {
                    if index < words {
                        self.write_word(ptr, u32::from_le_bytes(bytes[..4].try_into()?));
                        // The host controls random bytes. A path may use their length
                        // and return status but may not observe the bytes themselves.
                        if random {
                            self.random_words.insert(ptr);
                        }
                        bytes = &bytes[4..];
                        ptr += 4;
                    } else {
                        self.write_word(SAFE_WRITE + 4 * index as u32, 0);
                    }
                }
            } else {
                self.store_byte(ptr, bytes[0])?;
                bytes = &bytes[1..];
                ptr += 1;
            }
            self.cycles += 1;
        }
        self.store_register(10, data.len() as u32)?;
        Ok(())
    }

    fn sha2(
        &mut self,
        input: u32,
        output: u32,
        data: u32,
        count: u32,
        constants: u32,
    ) -> Result<()> {
        // The committed kernel uses the FIPS SHA-256 round constants.
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        ensure!(
            count <= 10
                && [input, output, data, constants]
                    .iter()
                    .all(|ptr| ptr & 3 == 0 && *ptr >= LOW && *ptr < u32::MAX - 1024),
            "invalid SHA call"
        );
        ensure!(
            output + 32 <= constants || output >= constants + 256,
            "SHA constants overlap output"
        );
        ensure!(
            input == output || input + 32 <= output || output + 32 <= input,
            "partially overlapping SHA state"
        );
        for (i, word) in K.into_iter().enumerate() {
            ensure!(
                self.read_word(constants + 4 * i as u32)? == word,
                "unknown SHA constants"
            );
        }
        let mut state = self.load_digest(input)?;
        self.store_digest(output, &state);
        for block in 0..count {
            let first = self.load_digest(data + 64 * block)?;
            let second = self.load_digest(data + 64 * block + 32)?;
            state = *Impl::compress(&state, &first, &second);
            self.store_digest(output, &state);
        }
        self.cycles += 6 + 68 * u64::from(count);
        Ok(())
    }
    fn load_digest(&self, address: u32) -> Result<Digest> {
        let mut words = [0; 8];
        for (i, word) in words.iter_mut().enumerate() {
            *word = self.read_word(address + 4 * i as u32)?;
        }
        Ok(Digest::from(words))
    }

    fn store_digest(&mut self, address: u32, digest: &Digest) {
        for (i, word) in digest.as_words().iter().enumerate() {
            self.write_word(address + 4 * i as u32, *word);
        }
    }
}

impl EmuContext for Machine<'_> {
    fn ecall(&mut self) -> Result<bool> {
        if self.mode == 0 {
            let destination = self.read_word(ECALL_DISPATCH)?;
            ensure!(
                destination & 3 == 0 && (KERNEL..0xff000000).contains(&destination),
                "invalid kernel dispatch"
            );
            self.write_word(MEPC, self.pc.0);
            self.pc = ByteAddr(destination);
            self.mode = 1;
            return Ok(true);
        }
        let [a0, a1, a2, a3, a4, opcode] =
            [10, 11, 12, 13, 14, 17].map(|index| self.load_register(index));
        let (a0, a1, a2, a3, a4, opcode) = (a0?, a1?, a2?, a3?, a4?, opcode?);
        match opcode {
            0 => {
                ensure!(a0 == 0, "unexpected actor termination {a0}");
                self.cycles += 2;
                self.exit = Some(Exit::Halted);
            }
            1 => self.host_read(a0, a1, a2)?,
            4 => self.sha2(a0, a1, a2, a3 & 0xffff, a4)?,
            _ => bail!("unsupported machine ecall {opcode} at {:#x}", self.pc.0),
        }
        self.pc = self.pc + 4u32;
        Ok(false)
    }

    fn mret(&mut self) -> Result<bool> {
        ensure!(self.mode == 1, "invalid mret");
        self.pc = ByteAddr(self.read_word(MEPC)?.checked_add(4).unwrap());
        self.mode = 0;
        Ok(true)
    }

    fn trap(&mut self, exception: Exception) -> Result<bool> {
        bail!("unsupported actor trap at {:#x}: {exception:?}", self.pc.0);
    }

    #[inline(always)]
    fn on_insn_decoded(&mut self, _: InsnKind, _: &DecodedInstruction) -> Result<()> {
        Ok(())
    }

    #[inline(always)]
    fn on_normal_end(&mut self, _: InsnKind) -> Result<()> {
        self.cycles += 1;
        Ok(())
    }

    #[inline(always)]
    fn get_pc(&self) -> ByteAddr {
        self.pc
    }
    #[inline(always)]
    fn set_pc(&mut self, addr: ByteAddr) {
        self.pc = addr;
    }

    #[inline(always)]
    fn load_register(&mut self, idx: usize) -> Result<u32> {
        if idx == 0 {
            return Ok(0);
        }
        Ok(self.registers[self.reg_index(idx)])
    }

    #[inline(always)]
    fn store_register(&mut self, idx: usize, value: u32) -> Result<()> {
        let index = self.reg_index(idx);
        if idx == 0 {
            self.write_word(MACHINE_REGS + 4 * index as u32 + 256, value);
        } else {
            self.registers[index] = value;
        }
        Ok(())
    }

    #[inline(always)]
    fn load_memory(&mut self, addr: WordAddr) -> Result<u32> {
        self.read_word(addr.baddr().0)
    }
    #[inline(always)]
    fn store_memory(&mut self, addr: WordAddr, value: u32) -> Result<()> {
        self.write_word(addr.baddr().0, value);
        Ok(())
    }

    #[inline(always)]
    fn check_insn_load(&self, addr: ByteAddr) -> bool {
        self.check_data_load(addr) && addr.0 & 3 == 0
    }
    #[inline(always)]
    fn check_data_load(&self, addr: ByteAddr) -> bool {
        addr.0 >= LOW && (self.mode != 0 || addr.0 < KERNEL)
    }
    #[inline(always)]
    fn check_data_store(&self, addr: ByteAddr) -> bool {
        self.check_data_load(addr)
    }
}
