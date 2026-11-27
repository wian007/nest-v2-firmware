// j49-screen: the thermostat screen of the Nest J49 with the public look. The app itself is the
// library next to this file (lib.rs); a skin of your own is a program like this one that gives
// `run` its own `Skin`.
fn main() {
    j49_screen::run(&j49_screen::DefaultSkin);
}
