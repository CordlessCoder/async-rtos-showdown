#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]

use stm32l0xx_hal as _; // memory layout

#[rtic::app(device = stm32l0xx_hal::pac, dispatchers = [USART2])]
mod app {
    use arrayvec::ArrayString;
    use core::fmt::Write;
    use embedded_time::rate::Hertz;
    use rtic_monotonics::fugit::Duration;
    use rtic_monotonics::stm32_tim2_monotonic;
    use rtic_time::Monotonic;
    use stm32l0xx_hal::exti::{Exti, ExtiLine, GpioLine};
    use stm32l0xx_hal::gpio::gpioa::PA5;
    use stm32l0xx_hal::gpio::gpiob::PB7;
    use stm32l0xx_hal::gpio::gpioc::PC13;
    use stm32l0xx_hal::gpio::{Floating, GpioExt, Input, Output, PushPull};
    use stm32l0xx_hal::prelude::{InputPin, OutputPin, ToggleableOutputPin};
    use stm32l0xx_hal::rcc::{self, RccExt};
    use stm32l0xx_hal::serial::{self, Serial};
    use stm32l0xx_hal::syscfg;
    use stm32l0xx_hal::timer::TimerExt;

    stm32_tim2_monotonic!(Timer, 32_768u16);

    #[shared]
    struct Shared {
        button_pressed: bool,
    }

    #[local]
    struct Local {
        btn: PC13<Input<Floating>>,
        tx: serial::Tx<serial::USART2>,
        led: PA5<Output<PushPull>>,
        button_processed: PB7<Output<PushPull>>,
        trigger_count: i32,
    }

    #[init]
    fn init(mut ctx: init::Context) -> (Shared, Local) {
        // Set up the system clock.
        let mut rcc = ctx.device.RCC.freeze(rcc::Config::default());
        Timer::start(32_768);

        let gpioa = ctx.device.GPIOA.split(&mut rcc);
        let gpiob = ctx.device.GPIOB.split(&mut rcc);
        let gpioc = ctx.device.GPIOC.split(&mut rcc);

        // Set up the LED.
        let led = gpioa.pa5.into_push_pull_output();
        let button_processed = gpiob.pb7.into_push_pull_output();

        // Set up the button.
        let btn = gpioc.pc13.into_floating_input();
        let mut sys_cfg = syscfg::SYSCFG::new(ctx.device.SYSCFG, &mut rcc);
        let mut exti = Exti::new(ctx.device.EXTI);
        exti.listen_gpio(
            &mut sys_cfg,
            stm32l0xx_hal::gpio::Port::PC,
            GpioLine::from_raw_line(13).unwrap(),
            stm32l0xx_hal::exti::TriggerEdge::Both,
        );

        // Set up serial.
        // let tx_pin = gpiod.pd8.into_alternate();
        // let tx: Tx<USART3, u8> =
        // Serial::tx(ctx.device.USART2, tx_pin, 115200i32.bps(), &rcc.clocks).unwrap();
        let usart = Serial::usart2(
            ctx.device.USART2,
            gpioa.pa2,
            gpioa.pa3,
            serial::Config::default(),
            &mut rcc,
        )
        .unwrap();
        let (tx, _) = usart.split();
        // let usart = Uart::new(
        // peri.USART2,
        // peri.PA3,
        // peri.PA2,
        // peri.DMA1_CH4,
        // peri.DMA1_CH5,
        // Irqs,
        // usart::Config::default(),
        // )

        rtic_monotonics::stm32::blink_led::spawn().ok();

        (
            Shared {
                button_pressed: false,
            },
            Local {
                btn,
                tx,
                led,
                button_processed,
                trigger_count: 0,
            },
        )
    }

    // Button interrupt
    #[task(binds = EXTI4_15, priority = 1, local = [btn], shared = [button_pressed])]
    fn on_exti(mut ctx: on_exti::Context) {
        process_button::spawn().ok();

        ctx.shared
            .button_pressed
            .lock(|button_pressed| *button_pressed = ctx.local.btn.is_high().unwrap());
    }

    #[task(priority = 0, local = [button_processed, trigger_count], shared = [button_pressed])]
    async fn process_button(mut ctx: process_button::Context) {
        ctx.local.button_processed.set_high().unwrap();

        *ctx.local.trigger_count += 1;

        write_serial::spawn(format_message(
            *ctx.local.trigger_count,
            ctx.shared
                .button_pressed
                .lock(|button_pressed| *button_pressed),
        ))
        .ok();

        ctx.local.button_processed.set_low().unwrap();
    }

    #[task(local = [tx])]
    async fn write_serial(ctx: write_serial::Context, msg: ArrayString<32>) {
        write!(ctx.local.tx, "{}", msg.as_str()).ok();
    }

    #[task(shared = [button_pressed], local = [led])]
    async fn blink_led(mut ctx: blink_led::Context) {
        let mut deadline = Timer::now();
        let interval = Duration::millis(100);
        loop {
            ctx.shared.button_pressed.lock(|button_pressed| {
                if *button_pressed {
                    ctx.local.led.set_low();
                } else {
                    ctx.local.led.toggle();
                }
            });
            deadline += interval;
            Timer::delay_until(deadline).await;
        }
    }

    fn format_message(trigger_count: i32, button_pressed: bool) -> ArrayString<32> {
        let mut string = ArrayString::new();
        core::writeln!(
            string,
            "Button is {} ({})",
            button_pressed as i32,
            trigger_count,
        )
        .unwrap();
        string
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
