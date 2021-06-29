## City17
A [Rust][rust] serverless function to retrieve and relay a playlist for Twitch livestreams/VODs.

By running this in specific countries and using [a browser extension][ext]
to redirect certain network requests to this function, Twitch will not display any ads.

I don't provide any pre-built version of this, you have to set it up yourself.

My rough estimate is that a few hundred to a few thousand users can be
supported while staying inside the free tier's limits, but I wouldn't recommend
publicizing your function unless you actually understand how the pricing works.
If you're the only user you won't need to pay anything. (Supposedly there's no
free tier on outbound bandwidth, but my bill in May was 0.000000 USD.)

You can probably run this as an actual server, but I haven't tested that because that
costs money.

<sup><sub>(I don't remember why I named this City17.)</sub></sup>

[rust]: https://www.rust-lang.org
[ext]: https://github.com/AlyoshaVasilieva/city17-ext

### Requirements

The function needs to be run in a country where Twitch doesn't serve ads.
Countries currently known: Russia, China.

I don't know of any serverless provider that operates in Russia, *and* supports Rust,
*and* allows use by people outside Russia. China has at least two providers that
should work fine, one of which I've personally tested. (Some amount of GFW evasion
takes place, since Twitch is blocked in China, but at least we can be pretty sure
that Twitch won't start running ads there...?)

I've only successfully run this on [Aliyun][ali] (Alibaba Cloud) from Chinese regions, and
the code will require modifications to run on a different provider. (Except Azure if I
managed to avoid breaking it after my initial tests there; there's a feature flag.)

Tencent Cloud should also work with some minor modifications, but I've been unable to
complete signup to test it.

Azure might work if [any of its locations][azure] are non-ad countries. Twitch considers
the UAE to be located inside the USA, so their Dubai location (UAE North) doesn't work.
Azure doesn't allow use of their Chinese regions unless you're a business with a presence
in China.

[ali]: https://www.alibabacloud.com/en
[azure]: https://azure.microsoft.com/en-us/global-infrastructure/services/?products=functions

### Building

If you're going to use Aliyun and do not intend to modify the code, download `city17.zip`
from [the latest release][release] and proceed to the setup instructions.

Run `build.sh`. Cannot be truly built in Windows due to *ring*, but `cargo check` and `cargo build`
work for checking the code. Ubuntu 20.04 via [WSL][wsl] works fine and is what I use.

Once `city17.zip` is built, see setup instructions below.

Requires:

* Rust [installed via rustup][rustup]
* The `x86_64-unknown-linux-musl` target for Rust
  (`rustup target add x86_64-unknown-linux-musl`)
* `7za` (`p7zip-full` on Debian and Ubuntu, can modify script to use normal zip command)
* `musl-gcc` (`musl-tools`)
* Probably `build-essential`

[release]: https://github.com/AlyoshaVasilieva/city17/releases/latest
[rustup]: https://rustup.rs/

### Aliyun setup instructions

1. Sign up, enable function compute, etc. (I'm not creating a new
   account just to write down all the steps)
2. Set the region to *China (Shanghai)*. As far as I know this is the
   region closest to Tokyo, which is where one of Twitch's servers is.
3. In the *Function Compute* menu, enter *Services and Functions* and create
   a service named `a`. (Or whatever you want, but you'll need to modify the code.)
4. Create a function:
   * HTTP
   * Named `prx`
   * Custom Runtime
   * Select the ZIP file `city17.zip`
   * Give it 128MB of RAM and a 15-second timeout.

For future updates of the function, use the Code tab's "Upload Zip File".
(Or get [fcli][fcli], set it up, and run `update.sh` after `build.sh`)

Under the Code tab, use the URL listed there to set up the browser extension.

It looks like `https://################.cn-shanghai.fc.aliyuncs.com/2016-08-15/proxy/a/prx/`;
you will need to add `invoke` to the end.

[fcli]: https://github.com/aliyun/fcli/releases
[wsl]: https://docs.microsoft.com/en-us/windows/wsl/install-win10

### Issues

* If the shell scripts fail due to having Windows line endings, run

```shell
dos2unix build.sh
dos2unix update.sh
```

in your Linux shell. (This shouldn't happen.)

### Extra reading

The [streamlink Twitch plugin][stp] has all the info needed in order to learn how to connect
to Twitch and get the M3U8 playlist. Some of it doesn't quite match what Twitch is doing now,
so watch a Twitch stream normally and look at the network requests your browser makes.

[stp]: https://github.com/streamlink/streamlink/blob/master/src/streamlink/plugins/twitch.py

### License

GNU GPLv3.
