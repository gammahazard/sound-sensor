//! Blink test — just blinks the LED on Pico 2 W
//! Based on embassy-rs rp235x examples

#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, cyw43_pio::PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Blink test starting...");
    let p = embassy_rp::init(Default::default());

    // CYW43 pin assignments per Pico 2 W schematic:
    // GPIO23 = WL_ON  (power enable)
    // GPIO24 = WL_D   (SPI data, bidirectional)
    // GPIO25 = WL_CS  (SPI chip select)
    // GPIO29 = WL_CLK (SPI clock)
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);

    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = cyw43_pio::PioSpi::new(
        &mut pio.common,
        pio.sm0,
        2u8.into(),
        pio.irq0,
        cs,       // PIN_25 = WL_CS
        p.PIN_24, // PIN_24 = WL_D
        p.PIN_29, // PIN_29 = WL_CLK
        p.DMA_CH0,
    );

    let fw = include_bytes!("../../firmware-rs/cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../firmware-rs/cyw43-firmware/43439A0_clm.bin");

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    let (_net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    info!("CYW43 initialized — blinking LED");

    // Blink forever: 500ms on, 500ms off
    loop {
        control.gpio_set(0, true).await;
        Timer::after(Duration::from_millis(500)).await;
        control.gpio_set(0, false).await;
        Timer::after(Duration::from_millis(500)).await;
    }
}
