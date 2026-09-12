//! Extract one supported audio elementary stream; never decode on the callback.
use crate::Result;
use mpeg2ts_reader::{
    StreamType,
    demultiplex::{self, DemuxContext, FilterRequest},
    packet, pes,
};

mpeg2ts_reader::packet_filter_switch! {
    Filter<Context> {
        Pat: demultiplex::PatPacketFilter<Context>,
        Pmt: demultiplex::PmtPacketFilter<Context>,
        Audio: pes::PesPacketFilter<Context, Consumer>,
        Ignore: demultiplex::NullPacketFilter<Context>,
    }
}
#[derive(Default)]
pub struct Context {
    changes: demultiplex::FilterChangeset<Filter>,
    selected: Option<packet::Pid>,
    output: Vec<u8>,
    broken: bool,
}
impl DemuxContext for Context {
    type F = Filter;
    fn filter_changeset(&mut self) -> &mut demultiplex::FilterChangeset<Filter> {
        &mut self.changes
    }
    fn construct(&mut self, request: FilterRequest<'_, '_>) -> Filter {
        match request {
            FilterRequest::ByPid(packet::Pid::PAT) => Filter::Pat(Default::default()),
            FilterRequest::Pmt {
                pid,
                program_number,
            } => Filter::Pmt(demultiplex::PmtPacketFilter::new(pid, program_number)),
            FilterRequest::ByStream {
                stream_type,
                stream_info,
                ..
            } if matches!(
                stream_type,
                StreamType::ADTS | StreamType::ISO_11172_AUDIO | StreamType::ISO_138183_AUDIO
            ) && self
                .selected
                .is_none_or(|p| p == stream_info.elementary_pid()) =>
            {
                self.selected = Some(stream_info.elementary_pid());
                Filter::Audio(pes::PesPacketFilter::new(Consumer))
            }
            _ => Filter::Ignore(Default::default()),
        }
    }
}
pub struct Consumer;
impl pes::ElementaryStreamConsumer<Context> for Consumer {
    fn start_stream(&mut self, _: &mut Context) {}
    fn begin_packet(&mut self, context: &mut Context, header: pes::PesHeader<'_>) {
        match header.contents() {
            pes::PesContents::Parsed(Some(p)) => context.output.extend_from_slice(p.payload()),
            pes::PesContents::Payload(p) => context.output.extend_from_slice(p),
            _ => context.broken = true,
        }
    }
    fn continue_packet(&mut self, context: &mut Context, bytes: &[u8]) {
        context.output.extend_from_slice(bytes);
    }
    fn end_packet(&mut self, _: &mut Context) {}
    fn continuity_error(&mut self, context: &mut Context) {
        context.broken = true;
    }
}
pub fn extract(bytes: &[u8]) -> Result<Vec<u8>> {
    if !bytes.len().is_multiple_of(188)
        || bytes
            .chunks(188)
            .any(|p| p[0] != 0x47 || p[1] & 0x80 != 0 || p[3] & 0xc0 != 0)
    {
        return Err("invalid, truncated, or scrambled MPEG-TS segment".into());
    }
    let mut context = Context::default();
    let mut demux = demultiplex::Demultiplex::new(&mut context);
    demux.push(&mut context, bytes);
    if context.broken || context.output.is_empty() {
        return Err("MPEG-TS segment has damaged or unsupported audio".into());
    }
    Ok(context.output)
}
