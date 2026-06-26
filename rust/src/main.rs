#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![allow(static_mut_refs)]

use core::sync::atomic::{AtomicBool, Ordering};

use arrayvec::ArrayString;
use defmt::println;
use embassy_executor::Spawner;
use embassy_stm32::dma;
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Level, Output, Pull, Speed};
use embassy_stm32::mode::Async;
use embassy_stm32::peripherals::{self};
use embassy_stm32::usart::{self, Uart};
use embassy_stm32::{Config, bind_interrupts, exti, interrupt};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::zerocopy_channel::{self};
use embassy_time::{Duration, Ticker};
use static_cell::make_static;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    EXTI13 => exti::InterruptHandler<interrupt::typelevel::EXTI13>;
    USART1 => usart::InterruptHandler<peripherals::USART1>;
    GPDMA1_CHANNEL0 => dma::InterruptHandler<peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => dma::InterruptHandler<peripherals::GPDMA1_CH1>;
    // TIM4 => embassy_stm32::timer::UpdateInterruptHandler<peripherals::TIM4>;
});

static mut UART_QUEUE_BUF: [ArrayString<32>; 8] = [ArrayString::new_const(); _];
static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);

#[embassy_executor::main(
    executor = "embassy_stm32::executor::Executor",
    entry = "cortex_m_rt::entry"
)]
async fn main(spawner: Spawner) {
    let mut config = Config::default();
    let peri = embassy_stm32::init(config);

    let led1 = Output::new(peri.PC7, Level::Low, Speed::VeryHigh);
    let button = ExtiInput::new(peri.PC13, peri.EXTI13, Pull::None, Irqs);
    let usart = Uart::new(
        peri.USART1,
        peri.PA10,
        peri.PA9,
        peri.GPDMA1_CH0,
        peri.GPDMA1_CH1,
        Irqs,
        usart::Config::default(),
    )
    .unwrap();


    let button_processed = Output::new(peri.PF14, Level::Low, Speed::VeryHigh);

    let (sender, receiver) = make_static!(
        zerocopy_channel::Channel::<'static, NoopRawMutex, _>::new(unsafe { &mut UART_QUEUE_BUF })
    )
    .split();

    spawner.spawn(blink_led(led1, &BUTTON_PRESSED).unwrap());
    spawner.spawn(uart_writer(usart, receiver).unwrap());
    spawner.spawn(button_waiter(button, &BUTTON_PRESSED, sender, button_processed).unwrap());
}

#[embassy_executor::task]
async fn blink_led(mut led: Output<'static>, button_high: &'static AtomicBool) {
    let mut ticker = Ticker::every(Duration::from_millis(100));
    loop {
        ticker.next().await;
        if !button_high.load(Ordering::SeqCst) {
            led.set_high();
        }
        ticker.next().await;
        led.set_low();
    }
}

#[embassy_executor::task]
async fn button_waiter(
    mut button: ExtiInput<'static, Async>,
    button_pressed: &'static AtomicBool,
    mut sender: zerocopy_channel::Sender<'static, NoopRawMutex, ArrayString<32>>,
    mut button_processed: Output<'static>,
) {
    let mut trigger_count = 0;

    fn format_message(buf: &mut ArrayString<32>, trigger_count: i32, button_pressed: bool) {
        use core::fmt::Write;

        buf.clear();
        core::writeln!(
            buf,
            "Button is {} ({})\n",
            button_pressed as i32,
            trigger_count,
        )
        .unwrap();
    }

    loop {
        button_processed.set_low();
        button.wait_for_rising_edge().await;
        button_processed.set_high();

        trigger_count += 1;
        button_pressed.store(true, Ordering::SeqCst);
        let mut slot = sender.send().await;
        format_message(&mut slot, trigger_count, true);
        slot.send_done();

        button_processed.set_low();
        button.wait_for_falling_edge().await;
        button_processed.set_high();

        trigger_count += 1;
        let mut slot = sender.send().await;
        format_message(&mut slot, trigger_count, false);
        slot.send_done();
        button_pressed.store(false, Ordering::SeqCst);
    }
}

#[embassy_executor::task]
async fn uart_writer(
    mut usart: Uart<'static, Async>,
    mut receiver: zerocopy_channel::Receiver<'static, NoopRawMutex, ArrayString<32>>,
) {
    loop {
        let message = receiver.receive().await;
        usart.write(message.as_bytes()).await.unwrap();
        message.receive_done();
    }
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    panic!("hardfault");
}
