#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![allow(static_mut_refs)]

use core::sync::atomic::{AtomicBool, Ordering};

use arrayvec::ArrayVec;
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
// use {defmt_rtt as _, panic_probe as _};
// use defmt::println;

bind_interrupts!(struct Irqs {
    EXTI4_15 => exti::InterruptHandler<interrupt::typelevel::EXTI4_15>;
    USART2 => usart::InterruptHandler<peripherals::USART2>;
    DMA1_CHANNEL4_5_6_7 =>
        dma::InterruptHandler<peripherals::DMA1_CH4>,
        dma::InterruptHandler<peripherals::DMA1_CH5>;
    // TIM4 => embassy_stm32::timer::UpdateInterruptHandler<peripherals::TIM4>;
});

type Buf = ArrayVec<u8, 32>;
type UartChannel = zerocopy_channel::Channel<'static, NoopRawMutex, Buf>;

static BUTTON_HIGH: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn main(spawner: Spawner, channel: &'static mut UartChannel) {
    let config = Config::default();
    let peri = embassy_stm32::init(config);

    let led1 = Output::new(peri.PA5, Level::Low, Speed::VeryHigh);
    let button = ExtiInput::new(peri.PC13, peri.EXTI13, Pull::None, Irqs);
    let usart = unsafe {
        Uart::new(
            peri.USART2,
            peri.PA3,
            peri.PA2,
            peri.DMA1_CH4,
            peri.DMA1_CH5,
            Irqs,
            usart::Config::default(),
        )
        .unwrap_unchecked()
    };

    let button_processed = Output::new(peri.PB7, Level::Low, Speed::VeryHigh);

    let (sender, receiver) = channel.split();

    spawner.spawn(blink_led(led1, &BUTTON_HIGH).unwrap());
    spawner.spawn(uart_writer(usart, receiver).unwrap());
    spawner.spawn(button_waiter(button, &BUTTON_HIGH, sender, button_processed).unwrap());
}

#[cortex_m_rt::entry]
/// SAFETY: Must only be called at most once
unsafe fn entry() -> ! {
    /// SAFETY: None
    #[inline(always)]
    unsafe fn make_static<T>(val: &mut T) -> &'static mut T {
        unsafe { core::mem::transmute(val) }
    }

    static mut UART_MSG_BUF: [Buf; 8] = [const { Buf::new_const() }; _];
    let mut channel =
        zerocopy_channel::Channel::<'static, NoopRawMutex, _>::new(unsafe { &mut UART_MSG_BUF });
    let channel = unsafe { make_static(&mut channel) };

    #[cfg(feature = "use-thread-executor")]
    {
        let mut executor = embassy_stm32::executor::Executor::new();
        let executor = unsafe { make_static(&mut executor) };
        executor.run(|s| main(s, channel));
    }
    #[cfg(not(feature = "use-thread-executor"))]
    {
        use embassy_stm32::executor::InterruptExecutor;
        use embassy_stm32::interrupt::InterruptExt;

        static EXECUTOR: InterruptExecutor = InterruptExecutor::new();

        /// LSD ISR → poll the executor. The peripheral itself is unused; we only borrow its interrupt vector as the executor's pend line
        #[interrupt]
        unsafe fn LCD() {
            unsafe {
                EXECUTOR.on_interrupt();
            }
        }

        interrupt::LCD.set_priority(interrupt::Priority::P7);
        let spawner = EXECUTOR.start(interrupt::LCD);
        // SAFETY: Sound ONLY if there is only 1 executor running
        let spawner: Spawner = unsafe { core::mem::transmute(spawner) };
        main(spawner, channel);
        loop {
            critical_section::with(|cs| unsafe {
                embassy_stm32::low_power::sleep(cs);
            });
        }
    }
}

#[embassy_executor::task]
async fn blink_led(mut led: Output<'static>, button_high: &'static AtomicBool) {
    let mut ticker = Ticker::every(Duration::from_millis(100));
    loop {
        ticker.next().await;
        if button_high.load(Ordering::SeqCst) {
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
    mut sender: zerocopy_channel::Sender<'static, NoopRawMutex, Buf>,
    mut button_processed: Output<'static>,
) {
    let mut trigger_count = 0;

    fn format_message(buf: &mut Buf, trigger_count: u32, button_pressed: bool) {
        buf.clear();
        unsafe {
            buf.try_extend_from_slice(b"Button is ").unwrap_unchecked();

            buf.push(b'0' + button_pressed as u8);
            buf.push(b'\n');
        }
    }

    loop {
        button_processed.set_low();
        button.wait_for_high().await;
        button_processed.set_high();

        trigger_count += 1;
        button_pressed.store(true, Ordering::SeqCst);
        let mut slot = sender.send().await;
        format_message(&mut slot, trigger_count, true);
        slot.send_done();

        button_processed.set_low();
        button.wait_for_low().await;
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
    mut receiver: zerocopy_channel::Receiver<'static, NoopRawMutex, Buf>,
) {
    loop {
        let message = receiver.receive().await;
        usart.write(&message).await.unwrap();
        message.receive_done();
    }
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    panic!("hardfault");
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::bkpt();
    }
}
