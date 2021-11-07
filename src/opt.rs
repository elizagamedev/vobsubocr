use clap::{crate_description, crate_name, crate_version, ArgEnum};
use clap::{Parser, ValueHint};
use std::path::PathBuf;

#[derive(ArgEnum, Copy, Clone, Debug)]
pub enum Script {
    Autodetect,
    SimplifiedChinese,
    TraditionalChinese,
}

#[derive(Parser, Debug)]
#[clap(name = crate_name!(), about = crate_description!(), version = crate_version!())]
pub struct Opt {
    /// Threshold for subtitle image binarization.
    ///
    /// Must be between 0.0 and 1.0. Only pixels with luminance above the
    /// threshold will be considered text pixels for OCR.
    #[clap(short = 't', long, default_value = "0.6")]
    pub threshold: f32,

    /// DPI of subtitle images.
    ///
    /// This setting doesn't strictly make sense for DVD subtitles, but it can
    /// influence Tesseract's output.
    #[clap(short = 'd', long, default_value = "150")]
    pub dpi: i32,

    /// Border in pixels to surround the each subtitle image for OCR.
    ///
    /// This can have subtle effects on the quality of the OCR.
    #[clap(short = 'b', long, default_value = "10")]
    pub border: u32,

    /// Output subtitle file.
    #[clap(short = 'o', long, parse(from_os_str), value_hint = ValueHint::FilePath)]
    pub output: PathBuf,

    /// Path to Tesseract's tessdata directory.
    #[clap(short = 'd', long, value_hint = ValueHint::DirPath)]
    pub tessdata: Option<String>,

    /// The Tesseract language code to use for OCR.
    ///
    /// Note that for Chinese simplified/traditional, specify only `chi`, not
    /// `chi_sim` or `chi_tra`. Use the `-s` option to specify the script
    /// instead.
    #[clap(short = 'l', long)]
    pub lang: String,

    /// A set of characters to blacklist from OCR.
    ///
    /// Tesseract can sometimes detect `|` as I, etc. Tuning the blacklist may
    /// yield better results.
    #[clap(short = 'b', long, default_value = "|\\/`_~")]
    pub blacklist: String,

    /// The character script that Tesseract will try to recognize.
    #[clap(arg_enum, short = 's', long, default_value = "autodetect")]
    pub script: Script,

    #[clap(name = "FILE", parse(from_os_str), value_hint = ValueHint::FilePath)]
    pub input: PathBuf,

    /// Dump processed subtitle images into the working directory.
    #[clap(long)]
    pub dump: bool,
}
