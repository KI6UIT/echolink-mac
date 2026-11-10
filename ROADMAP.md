# EchoLink for Mac: Roadmap

Where the project is going. This is a plan and a wish list, not a promise: items move as testing,
hardware and feedback allow. Anything marked *idea* hasn't been designed yet.

Current release: see the [latest release](https://github.com/KI6UIT/echolink-mac/releases/latest).
Ideas, bug reports and "I'd use this" votes are welcome under
[Issues](https://github.com/KI6UIT/echolink-mac/issues).

## Where we are

Already in the app: the full EchoLink client (directory, voice both ways, chat, proxy support), push-to-talk by
button, Space or a system-wide key, VOX, DTMF keypad, mic gain / Auto level / noise reduction / rumble filter /
compressor, swing-needle gauges (RX, MIC, LINK, COMP, NR), an adjustable jitter buffer, a contact log with ADIF
export, QRZ logbook upload and sync, QRZ callsign lookup, call control (block / allow lists, Do Not Disturb),
and a NETS tab for finding EchoLink nets.

## Next up: polish and distribution (0.3.x)

- Publish 0.3.1 (layout, NETS, VOX, speech processing, jitter buffer).
- **Signed and notarized builds** so macOS stops showing the "unidentified developer" warning (needs a paid
  Apple Developer ID). This also opens the door to automatic updates and a Homebrew cask
  (`brew install --cask echolink`).
- An **Intel Mac** download (today only Apple Silicon is built; Intel users build from source).
- Sort out the NetFinder arrangement for the NETS tab with its maintainers.
- Fixes from real-world testing: VOX, DTMF and the talk key on live repeaters, the LINK meter on Starlink and
  hotspot connections.

## The big one: Radio link mode (0.4 and beyond)

**Goal:** let the Mac be the bridge between a real radio and EchoLink, so people on the internet can reach your
local RF, and your local RF can reach the world. Today the app is a *client*: you talk with your own microphone.
Link mode makes it a *node* a radio is connected to.

This is a large feature with real hardware and legal questions, so it is broken into stages that each stand on
their own. Each stage ships only when it has been tested on a real radio.

### Stage 1: Personal radio link (simplex), your own callsign
Use your normal personal callsign to link one radio to EchoLink.
- **Radio audio**: pick a separate sound device for the radio (USB interface such as a Digirig, SignaLink or a
  radio's own USB audio) in addition to your headset, with separate RX/TX levels and the same gauges.
- **PTT control**: several ways to key the radio, chosen in Settings: serial RTS/DTR, CM108-style USB GPIO,
  CAT command, or VOX.
- **Squelch input (COS)**: know when the radio is receiving so its audio is sent to EchoLink only when there is
  something to send (serial CTS/DSR or CM108 input), with a VOX-style audio-gate fallback.
- **Safety basics**: transmit time-out timer, anti-kerchunk, and a periodic **station ID** (voice and/or CW)
  with the interval you set.
- Everything the app already does stays: NR, compressor, gauges, link quality.

### Stage 2: Rig control (CAT / CI-V)
- Talk to the radio's own control port to show and set **frequency, mode, power and S-meter**, and to key PTT by
  command. Candidate approach: [Hamlib](https://hamlib.github.io/) (open source, covers hundreds of radios),
  or a direct implementation for the radios people actually use.
- **Icom first** (IC-705, IC-7300 / 7300MK2, IC-9700, IC-7610), because that is the hardware we have already
  looked at, and many of those radios expose both audio and CI-V over one USB cable. Other brands follow via Hamlib.
- Show the radio's state in the app (a radio gauge or panel), and log the band and frequency in the contact
  log and ADIF automatically, replacing today's fixed "2m / FM" default.
- Frequency / tone presets for linking to a repeater.

### Stage 3: Link or repeater node (-L / -R) for a club or repeater
- Log in to the EchoLink directory as a **link (-L) or repeater (-R)** station, with its own status, location,
  frequency and tone shown in the directory.
- Needs the callsign to be validated for EchoLink node use; confirm the current requirements with EchoLink.
- **Remote control by DTMF** from the radio side: connect to a node number, disconnect, status, and an
  access-controlled command set (with a password or CTCSS gate).
- **Access control**: CTCSS/PL required to key the link, plus the existing block / allow lists.
- Courtesy tone and tail, voice announcements ("connected to ...", "disconnected"), and spoken status.
- Unattended-operation safeguards: stuck-PTT protection, auto-disconnect on idle, and a watchdog.

### Stage 4: Running it for others
- A small status view (who is connected, last heard, uptime) and a log of connections.
- Multiple simultaneous connections and conference behaviour.
- Remote start/stop and notifications, so a node on a spare Mac mini can be looked after from elsewhere.
- Optional scheduled actions (connect to a net at its start time, disconnect after).

### What link mode involves, in plain terms
- **Hardware**: a radio, a cable or interface that carries its audio and a way to key PTT, and ideally a separate
  sound device so the radio's audio and your headset don't mix. A spare Mac that can stay on is best for a node.
- **Software pieces**: second audio path and routing, PTT and COS drivers, rig control, an ID timer, a DTMF
  decoder, tone and voice generation, and node mode in the directory login.
- **Legal and operating rules**: you are responsible for what goes out on the air. Station identification, control
  operator, automatic or remote control rules, third-party traffic, and linking restrictions vary by country and
  license. For example, US operators should read FCC Part 97 on station ID and automatic / remote control. Check
  your own rules and your repeater coordinator's policy before linking. The app will help (ID timer, time-outs,
  access control) but cannot make a setup legal for you.
- **Testing**: this can't be fully proven without radios on the bench, so each stage will be tested on real
  hardware and released as "tested with ..." for the radios actually tried. Reports from other setups will be needed.

### Open questions
- Which interfaces to support first (USB sound card with CM108 GPIO, serial-keyed, CAT-only)?
- Hamlib versus a direct CI-V implementation for the first Icom radios.
- How much of a repeater controller (tones, timers, scripting) belongs in this app versus a dedicated controller.
- How to test unattended behaviour safely before anyone relies on it.

## Other ideas (not scheduled)

- Record received and transmitted audio to a file.
- Net reminders and auto-connect at a net's start time.
- A loss and jitter history strip for diagnosing Starlink and Wi-Fi dropouts, with alerts.
- A station detail card (location, tone, distance), and a nearby-stations filter by grid square.
- A menu bar mode and Dock badge for incoming calls.
- Gauge visibility toggles and more layouts.
- Accessibility: VoiceOver labels and full keyboard control.
- Translations.

## How you can help

- Try the app and tell us what's confusing or broken ([Issues](https://github.com/KI6UIT/echolink-mac/issues)).
- If you have a radio and an interface you'd like supported for link mode, say which, and whether you can test.
- Pull requests are welcome. The code is organized so the engine (`src/`) is separate from the screens
  (`src/bin/echolink-gui/`); see the README for how to build and test.
