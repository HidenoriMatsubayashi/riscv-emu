use crate::bus::bus::Bus;
use crate::bus::bus_fe310::BusFe310;
use crate::bus::bus_fu540::BusFu540;
use crate::bus::bus_qemu_virt::BusQemuVirt;
use crate::console::Console;
use crate::cpu::cpu::{Privilege, Xlen};
use crate::cpu::trap::*;
use crate::machine::Machine;
use std::collections::HashMap;

const PAGE_SIZE: u64 = 4096;

#[derive(Debug)]
pub enum AddressingMode {
    Bare,
    Sv32,
    Sv39,
    Sv48,
    Sv57,
    Sv64,
}

pub struct Mmu {
    pub bus: Box<dyn Bus>,
    xlen: Xlen,
    ppn: u64,
    addressing_mode: AddressingMode,
    privilege: Privilege,
    reserved_address: HashMap<u64, bool>,
}

struct Pte {
    ppn: u64, // physical page number
    ppns: [u64; 3],
    _rsw: u8, // reserved for use by supervisor software
    d: u8,    // dirty
    a: u8,    // accessed
    _g: u8,   // global mapping
    _u: u8,   // page is accessible to user mode
    x: u8,    // execute permission
    w: u8,    // write permission
    r: u8,    // read permission
    v: u8,    // PTE is valid
}

enum MemoryAccessType {
    Fetch,
    Read,
    Write,
}

impl Mmu {
    pub fn new(_xlen: Xlen, machine: Machine, console: Box<dyn Console>) -> Self {
        let machine_bus: Box<dyn Bus> = match machine {
            Machine::SiFiveE => Box::new(BusFe310::new(console)),
            Machine::SiFiveU => Box::new(BusFu540::new(console)),
            Machine::QemuVirt => Box::new(BusQemuVirt::new(console)),
        };
        Mmu {
            bus: machine_bus,
            xlen: _xlen,
            ppn: 0,
            addressing_mode: AddressingMode::Bare,
            privilege: Privilege::Machine,
            reserved_address: HashMap::new(),
        }
    }

    pub fn set_privilege(&mut self, privilege: &Privilege) {
        self.privilege = privilege.clone();
    }

    pub fn set_xlen(&mut self, xlen: &Xlen) {
        self.xlen = xlen.clone();
    }

    pub fn update_addressing_mode(&mut self, data: u64) {
        self.ppn = match self.xlen {
            Xlen::X64 => data & 0xfffffffffff,
            Xlen::X32 => data & 0x3fffff,
        };

        self.addressing_mode = match self.xlen {
            Xlen::X64 => match data >> 60 {
                0 => AddressingMode::Bare,
                8 => AddressingMode::Sv39,
                9 => AddressingMode::Sv48,
                n => panic!(" {:x} is not implemented yet.", n),
            },
            Xlen::X32 => match data & 0x80000000 {
                0 => AddressingMode::Bare,
                _ => AddressingMode::Sv32,
            },
        };
        //println!("update mode => {:?}", self.addressing_mode);
    }

    pub fn set_address_reserve(&mut self, addr: u64, request_reserve: bool) {
        match request_reserve {
            true => self.reserved_address.insert(addr, true),
            false => self.reserved_address.remove(&addr),
        };
    }

    pub fn is_address_reserved(&mut self, addr: u64) -> bool {
        match self.reserved_address.get_mut(&addr) {
            Some(_v) => true,
            _ => false,
        }
    }

    pub fn get_bus(&mut self) -> &mut Box<dyn Bus> {
        &mut self.bus
    }

    pub fn read8(&mut self, v_addr: u64) -> Result<u8, Trap> {
        let ev_addr = self.to_effective_address(v_addr);
        match self.to_physical_address(ev_addr, MemoryAccessType::Read) {
            Ok(p_addr) => match self.bus.read8(p_addr) {
                Ok(data) => Ok(data),
                Err(()) => Err(Trap {
                    exception: Exception::LoadPageFault,
                    value: ev_addr,
                }),
            },
            Err(()) => Err(Trap {
                exception: Exception::LoadPageFault,
                value: ev_addr,
            }),
        }
    }

    pub fn read16(&mut self, v_addr: u64) -> Result<u16, Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 2) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Read) {
                    Ok(p_addr) => match self.bus.read16(p_addr) {
                        Ok(data) => Ok(data),
                        Err(()) => Err(Trap {
                            exception: Exception::LoadPageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::LoadPageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                let mut data = 0 as u16;
                for i in 0..2 {
                    match self.read8(v_addr.wrapping_add(i)) {
                        Ok(d) => data |= (d as u16) << (i * 8),
                        Err(e) => return Err(e),
                    }
                }
                Ok(data)
            }
        }
    }

    pub fn read32(&mut self, v_addr: u64) -> Result<u32, Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 4) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Read) {
                    Ok(p_addr) => match self.bus.read32(p_addr) {
                        Ok(data) => Ok(data),
                        Err(()) => Err(Trap {
                            exception: Exception::LoadPageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::LoadPageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                let mut data = 0 as u32;
                for i in 0..4 {
                    match self.read8(v_addr.wrapping_add(i)) {
                        Ok(d) => data |= (d as u32) << (i * 8),
                        Err(e) => return Err(e),
                    }
                }
                Ok(data)
            }
        }
    }

    pub fn read32_direct(&mut self, p_addr: u64) -> Result<u32, Trap> {
        let ep_addr = self.to_effective_address(p_addr);
        match self.bus.read32(p_addr) {
            Ok(data) => Ok(data),
            Err(()) => Err(Trap {
                exception: Exception::LoadPageFault,
                value: ep_addr,
            }),
        }
    }

    pub fn read64(&mut self, v_addr: u64) -> Result<u64, Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 8) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Read) {
                    Ok(p_addr) => match self.bus.read64(p_addr) {
                        Ok(data) => Ok(data),
                        Err(()) => Err(Trap {
                            exception: Exception::LoadPageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::LoadPageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                let mut data = 0 as u64;
                for i in 0..8 {
                    match self.read8(v_addr.wrapping_add(i)) {
                        Ok(d) => data |= (d as u64) << (i * 8),
                        Err(e) => return Err(e),
                    }
                }
                Ok(data)
            }
        }
    }

    pub fn write8(&mut self, v_addr: u64, val: u8) -> Result<(), Trap> {
        let ev_addr = self.to_effective_address(v_addr);
        match self.to_physical_address(ev_addr, MemoryAccessType::Write) {
            Ok(p_addr) => match self.bus.write8(p_addr, val) {
                Ok(()) => Ok(()),
                Err(()) => Err(Trap {
                    exception: Exception::StorePageFault,
                    value: ev_addr,
                }),
            },
            Err(()) => Err(Trap {
                exception: Exception::StorePageFault,
                value: ev_addr,
            }),
        }
    }

    pub fn write16(&mut self, v_addr: u64, data: u16) -> Result<(), Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 2) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Write) {
                    Ok(p_addr) => match self.bus.write16(p_addr, data) {
                        Ok(()) => Ok(()),
                        Err(()) => Err(Trap {
                            exception: Exception::StorePageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::StorePageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                for i in 0..2 {
                    match self.write8(v_addr.wrapping_add(i), ((data >> (i * 8)) & 0xff) as u8) {
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
                Ok(())
            }
        }
    }

    pub fn write32(&mut self, v_addr: u64, data: u32) -> Result<(), Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 4) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Write) {
                    Ok(p_addr) => match self.bus.write32(p_addr, data) {
                        Ok(()) => Ok(()),
                        Err(()) => Err(Trap {
                            exception: Exception::StorePageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::StorePageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                for i in 0..4 {
                    match self.write8(v_addr.wrapping_add(i), ((data >> (i * 8)) & 0xff) as u8) {
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
                Ok(())
            }
        }
    }

    pub fn write64(&mut self, v_addr: u64, data: u64) -> Result<(), Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 8) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Write) {
                    Ok(p_addr) => match self.bus.write64(p_addr, data) {
                        Ok(()) => Ok(()),
                        Err(()) => Err(Trap {
                            exception: Exception::StorePageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::StorePageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                for i in 0..8 {
                    match self.write8(v_addr.wrapping_add(i), ((data >> (i * 8)) & 0xff) as u8) {
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
                Ok(())
            }
        }
    }

    pub fn fetch32(&mut self, v_addr: u64) -> Result<u32, Trap> {
        // sometimes access to unaliggned acccess.
        // If it exceeds the page size, it is necessary to refer to another page table.
        match v_addr & (PAGE_SIZE - 1) <= (PAGE_SIZE - 4) {
            true => {
                let ev_addr = self.to_effective_address(v_addr);
                match self.to_physical_address(ev_addr, MemoryAccessType::Fetch) {
                    Ok(p_addr) => match self.bus.read32(p_addr) {
                        Ok(data) => Ok(data),
                        Err(()) => Err(Trap {
                            exception: Exception::InstructionPageFault,
                            value: ev_addr,
                        }),
                    },
                    Err(()) => Err(Trap {
                        exception: Exception::InstructionPageFault,
                        value: ev_addr,
                    }),
                }
            }
            _ => {
                let mut data = 0 as u32;
                for i in 0..4 {
                    match self.fetch8(v_addr.wrapping_add(i)) {
                        Ok(d) => data |= (d as u32) << (i * 8),
                        Err(e) => return Err(e),
                    }
                }
                Ok(data)
            }
        }
    }

    /// Instruction fetch for unaliggned acccess when virtual addressing mode.
    fn fetch8(&mut self, v_addr: u64) -> Result<u8, Trap> {
        let ev_addr = self.to_effective_address(v_addr);
        match self.to_physical_address(ev_addr, MemoryAccessType::Fetch) {
            Ok(p_addr) => match self.bus.read8(p_addr) {
                Ok(data) => Ok(data),
                Err(()) => Err(Trap {
                    exception: Exception::InstructionPageFault,
                    value: ev_addr,
                }),
            },
            Err(()) => Err(Trap {
                exception: Exception::InstructionPageFault,
                value: ev_addr,
            }),
        }
    }

    fn to_physical_address(
        &mut self,
        v_addr: u64,
        access_type: MemoryAccessType,
    ) -> Result<u64, ()> {
        //println!("AddressingMode = {:?}", self.addressing_mode);
        match self.addressing_mode {
            AddressingMode::Bare => Ok(v_addr),
            AddressingMode::Sv32 => match self.privilege {
                Privilege::User | Privilege::Supervisor => {
                    let vpns = [(v_addr >> 12) & 0x3ff, (v_addr >> 22) & 0x3ff];
                    self.page_waking(v_addr, 1, self.ppn, &vpns, &access_type)
                }
                _ => Ok(v_addr),
            },
            AddressingMode::Sv39 => match self.privilege {
                Privilege::User | Privilege::Supervisor => {
                    let vpns = [
                        (v_addr >> 12) & 0x1ff,
                        (v_addr >> 21) & 0x1ff,
                        (v_addr >> 30) & 0x1ff,
                    ];
                    self.page_waking(v_addr, 2, self.ppn, &vpns, &access_type)
                }
                _ => Ok(v_addr),
            },
            AddressingMode::Sv48 => {
                panic!("AddressingMode SV48 is not implemented yet.");
            }
            AddressingMode::Sv57 => {
                panic!("AddressingMode SV57 is not implemented yet.");
            }
            AddressingMode::Sv64 => {
                panic!("AddressingMode SV64 is not implemented yet.");
            }
        }
    }

    fn page_waking(
        &mut self,
        v_addr: u64,
        level: u8,
        parent_ppn: u64,
        vpns: &[u64],
        access_type: &MemoryAccessType,
    ) -> Result<u64, ()> {
        // 1. calc PTE address.
        let pte_size = match self.addressing_mode {
            AddressingMode::Sv32 => 4,
            _ => 8,
        };
        let pte_addr = parent_ppn * PAGE_SIZE + vpns[level as usize] * pte_size;

        // 2. get PTE (Page Table Entry).
        let pte = match self.addressing_mode {
            AddressingMode::Sv32 => self.pte_read32(pte_addr) as u64,
            _ => self.pte_read64(pte_addr),
        };

        // 3. check PTE.
        let pte_d = self.parse_pte(pte);

        // 4. validate page-table. (PTE.V / PTE.R / PTE.W)
        if pte_d.v == 0 || (pte_d.r == 0 && pte_d.w == 1) {
            return Err(());
        }

        // 5. check last entry or not.
        if pte_d.r == 0 && pte_d.x == 0 {
            return match level {
                0 => Err(()),
                _ => self.page_waking(v_addr, level - 1, pte_d.ppn, vpns, access_type),
            };
        }

        // 6. check page-fault.
        if pte_d.a == 0
            || (match access_type {
                MemoryAccessType::Write => pte_d.d == 0,
                _ => false,
            })
        {
            let new_pte = pte
                | (1 << 6)
                | (match access_type {
                    MemoryAccessType::Write => 1 << 7,
                    _ => 0,
                });
            match self.addressing_mode {
                AddressingMode::Sv32 => self.pte_write32(pte_addr, new_pte as u32),
                _ => self.pte_write64(pte_addr, new_pte),
            };
            // return Err(()); need page-fault exception?
        }

        // 7. check access permission.
        match access_type {
            MemoryAccessType::Fetch => {
                if pte_d.x == 0 {
                    return Err(());
                }
            }
            MemoryAccessType::Read => {
                if pte_d.r == 0 {
                    return Err(());
                }
            }
            _ => {
                if pte_d.w == 0 {
                    return Err(());
                }
            }
        };

        // 8. calculate physical address.
        let offset = v_addr & 0xfff;
        let p_addr = match self.addressing_mode {
            AddressingMode::Sv32 => match level {
                1 => {
                    if pte_d.ppns[0] != 0 {
                        return Err(());
                    }
                    (pte_d.ppns[1] << 22) | (vpns[0] << 12) | offset
                }
                0 => (pte_d.ppn << 12) | offset,
                _ => panic!(),
            },
            _ => match level {
                2 => {
                    if pte_d.ppns[1] != 0 || pte_d.ppns[0] != 0 {
                        return Err(());
                    }
                    (pte_d.ppns[2] << 30) | (vpns[1] << 21) | (vpns[0] << 12) | offset
                }
                1 => {
                    if pte_d.ppns[0] != 0 {
                        return Err(());
                    }
                    (pte_d.ppns[2] << 30) | (pte_d.ppns[1] << 21) | (vpns[0] << 12) | offset
                }
                0 => (pte_d.ppn << 12) | offset,
                _ => panic!(),
            },
        };
        Ok(p_addr)
    }

    fn parse_pte(&self, pte: u64) -> Pte {
        let _ppn = match self.addressing_mode {
            AddressingMode::Sv32 => (pte >> 10) & 0x3fffff,
            _ => (pte >> 10) & 0xfff_ffffffff,
        };
        let _ppns = match self.addressing_mode {
            AddressingMode::Sv32 => [(pte >> 10) & 0x3ff, (pte >> 20) & 0xfff, 0],
            _ => [
                (pte >> 10) & 0x1ff,
                (pte >> 19) & 0x1ff,
                (pte >> 28) & 0x3ffffff,
            ],
        };
        Pte {
            ppn: _ppn,
            ppns: _ppns,
            _rsw: ((pte >> 8) & 0x3) as u8,
            d: ((pte >> 7) & 1) as u8,
            a: ((pte >> 6) & 1) as u8,
            _g: ((pte >> 5) & 1) as u8,
            _u: ((pte >> 4) & 1) as u8,
            x: ((pte >> 3) & 1) as u8,
            w: ((pte >> 2) & 1) as u8,
            r: ((pte >> 1) & 1) as u8,
            v: (pte & 1) as u8,
        }
    }

    fn pte_read32(&mut self, addr: u64) -> u32 {
        let effective_addr = self.to_effective_address(addr);
        match self.bus.read32(effective_addr) {
            Ok(data) => data,
            Err(e) => panic!(e),
        }
    }

    fn pte_read64(&mut self, addr: u64) -> u64 {
        let effective_addr = self.to_effective_address(addr);
        match self.bus.read64(effective_addr) {
            Ok(data) => data,
            Err(e) => panic!(e),
        }
    }

    fn pte_write32(&mut self, addr: u64, data: u32) {
        let effective_addr = self.to_effective_address(addr);
        match self.bus.write32(effective_addr, data) {
            Ok(()) => (),
            Err(e) => panic!(e),
        }
    }

    fn pte_write64(&mut self, addr: u64, data: u64) {
        let effective_addr = self.to_effective_address(addr);
        match self.bus.write64(effective_addr, data) {
            Ok(()) => (),
            Err(e) => panic!(e),
        }
    }

    fn to_effective_address(&self, addr: u64) -> u64 {
        match self.xlen {
            Xlen::X32 => addr & 0xffffffff,
            Xlen::X64 => addr,
        }
    }
}
