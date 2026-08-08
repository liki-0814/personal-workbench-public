use super::{IllustrationMode, IllustrationRequest};

pub fn resolve_mode(request: &IllustrationRequest) -> IllustrationMode {
    if request.mode != IllustrationMode::Auto {
        return request.mode;
    }
    let intent = request.visual_intent.to_ascii_lowercase();
    if !request.image_refs.is_empty() {
        if ["evaluate", "compare", "评估", "比较", "评分"]
            .iter()
            .any(|v| intent.contains(v))
        {
            return IllustrationMode::Eval;
        }
        if ["polish", "beautify", "美化", "润色", "风格"]
            .iter()
            .any(|v| intent.contains(v))
        {
            return IllustrationMode::Polish;
        }
        return IllustrationMode::Refine;
    }
    if request.content.is_array() || request.content.is_object() {
        return IllustrationMode::Plot;
    }
    IllustrationMode::Diagram
}
