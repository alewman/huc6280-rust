//! Directed tests of the crate's public surface that need no reference: the
//! bus contract (one callback a cycle, physical addresses, what the pins
//! show), the three interrupt lines and their disable register, the named
//! switches, and state capture and restore. The proof of equivalence with
//! huc6280-python is the ladder (`scripts/rung*.sh`), not these.

use huc6280::{
    Bus, CpuState, DecimalFormula, HuC6280, Kind, Pins, VanishedInterrupt, OPCODES, T, VDC_ADDRESS,
};

/// 2 MB of RAM that records every cycle with what the pins showed.
struct Ram {
    memory: Vec<u8>,
    cycles: Vec<(Kind, Option<u32>, bool)>,
    /// A line change to make during the cycle with this index: (IRQ1, IRQ2, TIQ).
    during: Option<(usize, (bool, bool, bool))>,
}

impl Bus for Ram {
    fn read(&mut self, address: u32, pins: &mut Pins) -> u8 {
        self.cycle(Some(address), pins);
        self.memory[address as usize]
    }
    fn write(&mut self, address: u32, value: u8, pins: &mut Pins) {
        self.cycle(Some(address), pins);
        self.memory[address as usize] = value;
    }
    fn idle(&mut self, address: Option<u32>, pins: &mut Pins) {
        self.cycle(address, pins);
    }
}

impl Ram {
    fn cycle(&mut self, address: Option<u32>, pins: &mut Pins) {
        if let Some((at, (irq1, irq2, tiq))) = self.during {
            if at == self.cycles.len() {
                pins.set_irq_lines(irq1, irq2, tiq);
            }
        }
        self.cycles.push((pins.kind(), address, pins.high_speed));
    }
}

const CODE: u32 = 0x1F0000; // work RAM, page $F8

/// A CPU with page $F8 at logical $4000 (code) and $2000 (zero page, stack),
/// bank 0 at $E000 (vectors), running `code` from $4000.
fn cpu(code: &[u8]) -> HuC6280<Ram> {
    let mut memory = vec![0; 0x200000];
    memory[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
    let mut cpu = HuC6280::new(Ram {
        memory,
        cycles: Vec::new(),
        during: None,
    });
    cpu.mpr = [0xFF, 0xF8, 0xF8, 0, 0, 0, 0, 0];
    cpu.pc = 0x4000;
    cpu
}

fn vector(cpu: &mut HuC6280<Ram>, vector: u16, target: u16) {
    let at = cpu.physical(vector) as usize;
    cpu.bus.memory[at] = target as u8;
    cpu.bus.memory[at + 1] = (target >> 8) as u8;
}

#[test]
fn every_cycle_is_one_callback_with_its_kind() {
    // LDA $10: fetch, operand, internal, read of the zero page at physical $1F0010.
    let mut cpu = cpu(&[0xA5, 0x10]);
    cpu.bus.memory[0x1F0010] = 0x80;
    assert_eq!(cpu.step(), 4);
    assert_eq!(
        cpu.bus.cycles,
        [
            (Kind::Read, Some(CODE), false),
            (Kind::Read, Some(CODE + 1), false),
            (Kind::Idle, None, false),
            (Kind::Read, Some(0x1F0010), false),
        ]
    );
    assert_eq!((cpu.a, cpu.p & 0x82), (0x80, 0x80));
    assert_eq!(cpu.opcode(), 0xA5);
}

#[test]
fn the_table_has_every_opcode_and_its_base_cycles_are_what_runs() {
    // Every opcode but the block transfers and BRK, from the same state: T
    // and D clear, branches not taken where the flags say so.
    let mut measured = 0;
    for (opcode, row) in OPCODES.iter().enumerate() {
        let (_, _, mnemonic, form, cycles) = *row;
        let taken = matches!(mnemonic, "BPL" | "BVC" | "BCC" | "BNE");
        if form == huc6280::Form::Block || taken || mnemonic.starts_with("BBR") {
            continue;
        }
        let mut cpu = cpu(&[opcode as u8, 0x10, 0x30, 0x00]);
        assert_eq!(cpu.step(), cycles as u32, "opcode {opcode:02X} {mnemonic}");
        measured += 1;
    }
    assert_eq!(measured, 256 - 5 - 4 - 8);
}

#[test]
fn st0_writes_the_vdc_whatever_the_mapper_holds() {
    let mut cpu = cpu(&[0x03, 0x5A]);
    cpu.mpr = [0xF8; 8]; // no register holds $FF, the hardware page
    cpu.step();
    assert_eq!(cpu.bus.memory[VDC_ADDRESS as usize], 0x5A);
    assert_eq!(cpu.bus.cycles[3], (Kind::Write, Some(VDC_ADDRESS), false));
}

#[test]
fn a_block_transfer_is_one_step_of_17_plus_6n_cycles() {
    // TII $2200,$2240,3
    let mut cpu = cpu(&[0x73, 0x00, 0x22, 0x40, 0x22, 0x03, 0x00]);
    cpu.bus.memory[0x1F0200..0x1F0203].copy_from_slice(&[1, 2, 3]);
    assert_eq!(cpu.step(), 17 + 6 * 3);
    assert_eq!(cpu.bus.memory[0x1F0240..0x1F0243], [1, 2, 3]);
    assert_eq!(cpu.pc, 0x4007);
}

#[test]
fn csh_holds_for_its_own_last_cycle_and_reset_selects_low() {
    let mut cpu = cpu(&[0xD4]);
    cpu.step();
    assert!(cpu.high_speed());
    let speeds: Vec<bool> = cpu.bus.cycles.iter().map(|cycle| cycle.2).collect();
    assert_eq!(speeds, [false, false, true]); // C7
    cpu.request_reset();
    cpu.step();
    assert!(!cpu.high_speed());
    assert_eq!(cpu.mpr[7], 0);
}

#[test]
fn set_makes_adc_work_on_the_zero_page_and_the_fetch_clears_t() {
    // SET; ADC #$05 with X = $10: M($2010) += 5, A untouched, three more cycles.
    let mut cpu = cpu(&[0xF4, 0x69, 0x05]);
    cpu.x = 0x10;
    cpu.a = 0x77;
    cpu.bus.memory[0x1F0010] = 0x20;
    cpu.step();
    assert_eq!(cpu.p & T, T);
    assert_eq!(cpu.step(), 2 + 3);
    assert_eq!(
        (cpu.bus.memory[0x1F0010], cpu.a, cpu.p & T),
        (0x25, 0x77, 0)
    );
}

#[test]
fn the_decimal_formulas_differ_only_on_invalid_digits() {
    let p = 0x08; // D
    for (a, operand) in [(0x19, 0x28), (0x99, 0x01), (0x50, 0x50)] {
        assert_eq!(
            huc6280::add(a, operand, p, DecimalFormula::Mednafen),
            huc6280::add(a, operand, p, DecimalFormula::Mesen)
        );
    }
    assert_eq!(
        huc6280::add(0x19, 0x28, p, DecimalFormula::Mednafen).0,
        0x47
    );
    assert_ne!(
        huc6280::add(0x0F, 0x0F, p, DecimalFormula::Mednafen),
        huc6280::add(0x0F, 0x0F, p, DecimalFormula::Mesen)
    );
}

#[test]
fn the_three_lines_have_their_vectors_priority_and_disable_bits() {
    for (lines, disable, target) in [
        ((true, true, true), 0, 0x5000),   // TIQ first
        ((true, true, false), 0, 0x5100),  // then IRQ1
        ((false, true, false), 0, 0x5200), // then IRQ2
        ((true, true, true), 4, 0x5100),   // the timer masked
        ((true, true, true), 6, 0x5200),   // the timer and IRQ1 masked
    ] {
        let mut cpu = cpu(&[0xEA]);
        vector(&mut cpu, 0xFFFA, 0x5000);
        vector(&mut cpu, 0xFFF8, 0x5100);
        vector(&mut cpu, 0xFFF6, 0x5200);
        cpu.p = 0;
        cpu.set_irq_disable(disable);
        cpu.set_irq_lines(lines.0, lines.1, lines.2);
        assert_eq!(cpu.step(), 8); // C3
        assert_eq!(cpu.pc, target);
        assert_eq!(cpu.p & 0x0C, 0x04); // I set, D clear
        assert_eq!(cpu.irq_lines(), lines); // levels: taking one does not clear it
    }
    // All three masked: nothing is taken, the lines are remembered.
    let mut cpu = cpu(&[0xEA]);
    cpu.p = 0;
    cpu.set_irq_disable(7);
    cpu.set_irq_lines(true, true, true);
    assert_eq!(cpu.step(), 2);
    assert_eq!(cpu.pc, 0x4001);
}

#[test]
fn a_line_raised_in_the_last_cycle_waits_one_instruction() {
    // NOP NOP NOP; IRQ1 rises during the first NOP's last cycle (cycle 1).
    let mut cpu = cpu(&[0xEA, 0xEA, 0xEA]);
    vector(&mut cpu, 0xFFF8, 0x5100);
    cpu.p = 0;
    cpu.bus.during = Some((1, (true, false, false)));
    cpu.step();
    cpu.step(); // the second NOP still runs
    assert_eq!(cpu.pc, 0x4002);
    cpu.step();
    assert_eq!(cpu.pc, 0x5100);
}

#[test]
fn a_timer_interrupt_whose_source_has_gone_is_dropped_or_latched() {
    for (mode, entered) in [
        (VanishedInterrupt::Mesen2, false),
        (VanishedInterrupt::Latched, true),
        (VanishedInterrupt::Drop, false),
    ] {
        // CLI; NOP; NOP with the timer's line up: CLI's poll still sees I
        // set, the NOP's poll sees the line, and the line drops in the NOP's
        // last cycle (C18).
        let mut cpu = cpu(&[0x58, 0xEA, 0xEA]);
        vector(&mut cpu, 0xFFFA, 0x5000);
        cpu.vanished_interrupt = mode;
        cpu.set_irq_lines(false, false, true);
        cpu.bus.during = Some((3, (false, false, false)));
        cpu.step();
        cpu.step();
        assert_eq!(cpu.pc, 0x4002);
        cpu.step();
        assert_eq!(cpu.pc == 0x5000, entered, "{mode:?}");
    }
}

#[test]
fn a_state_is_captured_and_restored_whole() {
    let mut cpu = cpu(&[0x58, 0xD4, 0xA9, 0x12, 0x53, 0x04]); // CLI; CSH; LDA #$12; TAM #$04
    cpu.set_irq_disable(5);
    cpu.set_irq_lines(false, true, true);
    for _ in 0..4 {
        cpu.step();
    }
    let state = cpu.capture_state();
    assert_eq!(
        (state.a, state.mpr[2], state.mpr_latch, state.high_speed),
        (0x12, 0x12, 0x12, true)
    );
    assert_eq!(
        (state.irq1, state.irq2, state.tiq, state.irq_disable),
        (false, true, true, 5)
    );
    assert!(!state.maskable_interrupt_pending); // IRQ2 and TIQ are both masked

    let mut other = HuC6280::new(Ram {
        memory: Vec::new(),
        cycles: Vec::new(),
        during: None,
    });
    other.restore_state(&state).unwrap();
    assert_eq!(other.capture_state(), state);

    let bad = CpuState {
        p: 0x10,
        ..CpuState::default()
    };
    assert!(other.restore_state(&bad).is_err()); // B is not stored
}
