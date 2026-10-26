use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::def::PortAvailable;
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::{Operation, State as OperationState};

use crate::device::{Card, Device, Profile};
use crate::error::{Error, Result};

pub const APP_NAME: &str = "audio";

/// How long to wait for devices to reappear after a Bluetooth profile change.
const PROFILE_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub struct Defaults {
    pub sink: Option<String>,
    pub source: Option<String>,
}

/// A blocking wrapper around a libpulse context.
pub struct Backend {
    // Declared before `mainloop` so the context is dropped first.
    context: Context,
    mainloop: Mainloop,
}

impl Backend {
    pub fn connect() -> Result<Self> {
        let mut mainloop = Mainloop::new().ok_or(Error::Connect)?;
        let mut context = Context::new(&mainloop, APP_NAME).ok_or(Error::Connect)?;
        context
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|_| Error::Connect)?;
        loop {
            iterate(&mut mainloop)?;
            match context.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => return Err(Error::Connect),
                _ => {}
            }
        }
        Ok(Self { context, mainloop })
    }

    pub fn defaults(&mut self) -> Result<Defaults> {
        let out = Rc::new(RefCell::new(Defaults::default()));
        let cell = Rc::clone(&out);
        let op = self.context.introspect().get_server_info(move |info| {
            *cell.borrow_mut() = Defaults {
                sink: info.default_sink_name.as_deref().map(str::to_owned),
                source: info.default_source_name.as_deref().map(str::to_owned),
            };
        });
        self.wait(op, "get server info")?;
        Ok(out.take())
    }

    pub fn sinks(&mut self) -> Result<Vec<Device>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let cell = Rc::clone(&out);
        let op = self.context.introspect().get_sink_info_list(move |result| {
            if let ListResult::Item(info) = result
                && let Some(name) = info.name.as_deref()
            {
                let description = info.description.as_deref().unwrap_or(name);
                let available = plugged(info.ports.iter().map(|p| p.available));
                cell.borrow_mut().push(Device::new(
                    info.index,
                    name,
                    description,
                    info.card,
                    available,
                ));
            }
        });
        self.wait(op, "list outputs")?;
        Ok(out.take())
    }

    /// Lists real inputs, skipping the monitors of outputs.
    pub fn sources(&mut self) -> Result<Vec<Device>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let cell = Rc::clone(&out);
        let op = self
            .context
            .introspect()
            .get_source_info_list(move |result| {
                if let ListResult::Item(info) = result
                    && info.monitor_of_sink.is_none()
                    && let Some(name) = info.name.as_deref()
                {
                    let description = info.description.as_deref().unwrap_or(name);
                    let available = plugged(info.ports.iter().map(|p| p.available));
                    cell.borrow_mut().push(Device::new(
                        info.index,
                        name,
                        description,
                        info.card,
                        available,
                    ));
                }
            });
        self.wait(op, "list inputs")?;
        Ok(out.take())
    }

    /// Lists Bluetooth cards with their profiles.
    pub fn bluetooth_cards(&mut self) -> Result<Vec<Card>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let cell = Rc::clone(&out);
        let op = self.context.introspect().get_card_info_list(move |result| {
            let ListResult::Item(info) = result else {
                return;
            };
            let Some(name) = info.name.as_deref().filter(|n| n.starts_with("bluez_")) else {
                return;
            };
            let profiles = info
                .profiles
                .iter()
                .filter_map(|p| {
                    Some(Profile {
                        name: p.name.as_deref()?.to_owned(),
                        priority: p.priority,
                        available: p.available,
                    })
                })
                .collect();
            cell.borrow_mut().push(Card {
                index: info.index,
                name: name.to_owned(),
                active_profile: info
                    .active_profile
                    .as_ref()
                    .and_then(|p| p.name.as_deref())
                    .map(str::to_owned),
                profiles,
            });
        });
        self.wait(op, "list cards")?;
        Ok(out.take())
    }

    pub fn set_default_sink(&mut self, name: &str) -> Result<()> {
        let ok = Rc::new(Cell::new(false));
        let flag = Rc::clone(&ok);
        let op = self
            .context
            .set_default_sink(name, move |success| flag.set(success));
        self.wait_success(op, &ok, "set default output")
    }

    pub fn set_default_source(&mut self, name: &str) -> Result<()> {
        let ok = Rc::new(Cell::new(false));
        let flag = Rc::clone(&ok);
        let op = self
            .context
            .set_default_source(name, move |success| flag.set(success));
        self.wait_success(op, &ok, "set default input")
    }

    pub fn set_card_profile(&mut self, card: &str, profile: &str) -> Result<()> {
        let ok = Rc::new(Cell::new(false));
        let flag = Rc::clone(&ok);
        let op = self.context.introspect().set_card_profile_by_name(
            card,
            profile,
            Some(Box::new(move |success| flag.set(success))),
        );
        self.wait_success(op, &ok, "set Bluetooth profile")
    }

    /// Moves every playing stream to the given output.
    pub fn move_playback(&mut self, sink: &str) -> Result<()> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let cell = Rc::clone(&out);
        let op = self
            .context
            .introspect()
            .get_sink_input_info_list(move |result| {
                if let ListResult::Item(info) = result {
                    cell.borrow_mut().push(info.index);
                }
            });
        self.wait(op, "list playback streams")?;
        for index in out.take() {
            // Some streams refuse to move (e.g. DONT_MOVE); that is not fatal.
            let op = self
                .context
                .introspect()
                .move_sink_input_by_name(index, sink, None);
            self.wait(op, "move playback stream")?;
        }
        Ok(())
    }

    /// Moves every recording stream that uses a real input to the given input.
    /// Streams recording an output monitor (e.g. screen capture audio) are left alone.
    pub fn move_recording(&mut self, source: &str) -> Result<()> {
        let inputs: Vec<u32> = self.sources()?.iter().map(|d| d.index).collect();
        let out = Rc::new(RefCell::new(Vec::new()));
        let cell = Rc::clone(&out);
        let op = self
            .context
            .introspect()
            .get_source_output_info_list(move |result| {
                if let ListResult::Item(info) = result {
                    cell.borrow_mut().push((info.index, info.source));
                }
            });
        self.wait(op, "list recording streams")?;
        for (index, current) in out.take() {
            if !inputs.contains(&current) {
                continue;
            }
            let op = self
                .context
                .introspect()
                .move_source_output_by_name(index, source, None);
            self.wait(op, "move recording stream")?;
        }
        Ok(())
    }

    /// Waits for the output of a card to appear after a profile change.
    pub fn wait_for_sink(&mut self, card: u32) -> Result<Device> {
        self.poll(|backend| Ok(backend.sinks()?.into_iter().find(|d| d.card == Some(card))))
    }

    /// Waits for the input of a card to appear after a profile change.
    pub fn wait_for_source(&mut self, card: u32) -> Result<Device> {
        self.poll(|backend| {
            Ok(backend
                .sources()?
                .into_iter()
                .find(|d| d.card == Some(card)))
        })
    }

    fn poll<T>(&mut self, mut find: impl FnMut(&mut Self) -> Result<Option<T>>) -> Result<T> {
        let deadline = Instant::now() + PROFILE_TIMEOUT;
        loop {
            if let Some(found) = find(self)? {
                return Ok(found);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn wait<T: ?Sized>(&mut self, op: Operation<T>, what: &'static str) -> Result<()> {
        loop {
            match op.get_state() {
                OperationState::Done => return Ok(()),
                OperationState::Cancelled => return Err(Error::Operation(what)),
                OperationState::Running => iterate(&mut self.mainloop)?,
            }
        }
    }

    fn wait_success<T: ?Sized>(
        &mut self,
        op: Operation<T>,
        ok: &Cell<bool>,
        what: &'static str,
    ) -> Result<()> {
        self.wait(op, what)?;
        if ok.get() {
            Ok(())
        } else {
            Err(Error::Operation(what))
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

/// A device is usable unless all of its ports report an unplugged jack.
/// Devices without ports, or without jack detection, count as plugged in.
fn plugged(ports: impl IntoIterator<Item = PortAvailable>) -> bool {
    let mut ports = ports.into_iter().peekable();
    ports.peek().is_none() || ports.any(|available| available != PortAvailable::No)
}

fn iterate(mainloop: &mut Mainloop) -> Result<()> {
    match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(_) | IterateResult::Err(_) => {
            Err(Error::Operation("mainloop iteration"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugged_unless_every_port_is_unplugged() {
        use PortAvailable::{No, Unknown, Yes};
        assert!(plugged([]));
        assert!(plugged([Unknown]));
        assert!(plugged([No, Yes]));
        assert!(!plugged([No]));
        assert!(!plugged([No, No, No]));
    }
}
