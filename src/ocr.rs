use crate::{opt::Opt, preprocessor::PreprocessedVobSubtitle};
use image::{
    pnm::{PNMSubtype, SampleEncoding},
    DynamicImage, GrayImage,
};
use rayon::prelude::*;
use scoped_tls_hkt::scoped_thread_local;
use snafu::{ResultExt, Snafu};
use subparse::timetypes::TimeSpan;
use tesseract::{OcrEngineMode, Tesseract};

scoped_thread_local!(static mut TESSERACT: TesseractWrapper);

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Could not build tesseract thread pool: {}", source))]
    BuildThreadPool { source: rayon::ThreadPoolBuildError },

    #[snafu(display("Could not initialize tesseract {}", source))]
    Initialize { source: tesseract::InitializeError },

    #[snafu(display("Could not set tesseract variable: {}", source))]
    SetVariable { source: tesseract::SetVariableError },

    #[snafu(display("Could not write image to memory: {}", source))]
    WriteImage { source: image::ImageError },

    #[snafu(display("Could not set tesseract image: {}", source))]
    SetImage {
        source: tesseract::plumbing::leptonica_plumbing::PixReadMemError,
    },

    #[snafu(display("Could not get tesseract text: {}", source))]
    GetText {
        source: tesseract::plumbing::TessBaseApiGetUtf8TextError,
    },

    #[snafu(display("Tesseract not initialized"))]
    TesseractNotInitialized,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn process(
    vobsubs: Vec<PreprocessedVobSubtitle>,
    opt: &Opt,
) -> Result<Vec<Result<(TimeSpan, String)>>> {
    std::env::set_var("OMP_THREAD_LIMIT", "1");
    Ok(rayon::ThreadPoolBuilder::new()
        .build_scoped(
            |thread| {
                let mut tesseract = TesseractWrapper::new(
                    opt.tessdata.clone(),
                    opt.lang.clone(),
                    opt.blacklist.clone(),
                );
                TESSERACT.set(&mut tesseract, || thread.run())
            },
            |pool| {
                pool.install(|| {
                    vobsubs
                        .into_par_iter()
                        .map(|vobsub| {
                            let text = vobsub
                                .images
                                .into_iter()
                                .map(|image| {
                                    TESSERACT.with(|tesseract| {
                                        tesseract.set_image(image, opt.dpi)?;
                                        Ok(tesseract.get_text()?)
                                    })
                                })
                                .collect::<Result<String>>()?;
                            Ok((vobsub.time_span, text))
                        })
                        .collect::<Vec<Result<(TimeSpan, String)>>>()
                })
            },
        )
        .context(BuildThreadPool {})?)
}

struct TesseractWrapper {
    datapath: Option<String>,
    language: String,
    blacklist: String,
    tesseract: Option<Tesseract>,
}

impl TesseractWrapper {
    fn new(datapath: Option<String>, language: String, blacklist: String) -> Self {
        Self {
            datapath,
            language,
            blacklist,
            tesseract: None,
        }
    }

    /// Set the tesseract image to the given image's contents.
    fn set_image(&mut self, image: GrayImage, dpi: i32) -> Result<()> {
        let mut bytes: Vec<u8> = Vec::new();
        DynamicImage::ImageLuma8(image)
            .write_to(
                &mut bytes,
                image::ImageOutputFormat::Pnm(PNMSubtype::Graymap(SampleEncoding::Binary)),
            )
            .context(WriteImage {})?;
        self.with_tesseract(|tesseract| {
            Ok(tesseract
                .set_image_from_mem(&bytes)
                .context(SetImage {})?
                .set_source_resolution(dpi))
        })?;
        Ok(())
    }

    /// Get text.
    fn get_text(&mut self) -> Result<String> {
        Ok(self
            .tesseract
            .as_mut()
            .ok_or(Error::TesseractNotInitialized)?
            .get_text()
            .context(GetText {})?)
    }

    /// Run mutable tesseract code without angering the rust compiler.
    fn with_tesseract(&mut self, func: impl FnOnce(Tesseract) -> Result<Tesseract>) -> Result<()> {
        let tesseract = match self.tesseract.take() {
            Some(tesseract) => tesseract,
            None => Tesseract::new_with_oem(
                self.datapath.as_deref(),
                Some(&self.language),
                OcrEngineMode::LstmOnly,
            )
            .context(Initialize {})?
            .set_variable("classify_enable_learning", "0")
            .context(SetVariable {})?
            .set_variable("tessedit_char_blacklist", &self.blacklist)
            .context(SetVariable {})?,
        };
        self.tesseract = Some(func(tesseract)?);
        Ok(())
    }
}
