// my awesome lrc parser
// probably breaks on real files but it's good enough for now

pub fn parse_line(line: &str) -> Option<(usize, String)> {
    if let Some(start) = line.find('[') {
        if let Some(end) = line.find(']') {
            if let Some(sep) = line.find(':') {
                if let Some(dot) = line.find('.') {
                    if !(start < sep
                        && end > sep
                        && start < dot
                        && end > dot
                        && start < end
                        && sep < dot)
                    {
                        return None;
                    }
                    // perhaps error prone
                    let minutes = &line[start + 1..sep];
                    let seconds = &line[sep + 1..dot];
                    let hundredths = &line[dot + 1..end];
                    let minutes: usize = minutes.parse().ok()?;
                    let seconds: usize = seconds.parse().ok()?;
                    let hundredths: usize = hundredths.parse().ok()?;
                    let sum = minutes * 60000 + seconds * 1000 + hundredths * 10;
                    let lyric = &line[end + 1..];
                    return Some((sum, lyric.to_owned()));
                }
            }
        }
    }
    None
}
