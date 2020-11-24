## Brewing Fridge Controller

This is a beer brewing fridge control program with an integrated web interface.

### Web Interface

The web interface is responsive on a phone, using `mousedown`/`touchstart`. 
It's very satisfying to hear the fridge starting *wom* the instant you press Save.
Authentication is hardcoded in the config file, based on everlasting browser
session cookies. Unauthenticated users will see a "Register" link to email the
site owner (set in the [config file](src/defconfig.toml)).

You can try a [static copy](https://matt.ucc.asn.au/ferment.html) of the interface.

I'm currently using Telegraf/InfluxDB/Grafana to graph temperatures, pulling from the `/status` json url.

### Hardware
I'm running it on a Raspberry Pi with ds18b20 1-wire sensors. The fridge
is turned on and off via a GPIO pin (and external AC switch).

Compile it by getting the necessary targets with rustup then `cargo build --release --target arm-unknown-linux-musleabihf`

### Older Version

The previous incarnation [wort-templog](https://github.com/mkj/wort-templog)
was written in Python with the web interface on a separate server. Colocating
the webserver on the control device reduces the number of moving parts.

It remains to be seen whether this Rust rewrite is more reliable than its predecessor.
Wort temperature control is an important matter!

### TODO

* Try better temperature control algorithms, take account of fridge air temperature for overshoot
* Pass through `/.well-known` for letsencrypt certbot (or find a rust acme crate that works with rustls and handle it internally)
