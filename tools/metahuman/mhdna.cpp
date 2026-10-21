// mhdna: dump a MetaHuman DNA (2.5, as UE 5.7 writes it) and evaluate RigLogic on it.
//
//   mhdna dump <file.dna> <out>           <out>.json (index) + <out>.bin (raw little-endian arrays)
//   mhdna eval <file.dna> <poses.txt> <out>
//
// poses.txt: one "pose control value" triple per line (control names case-insensitive, without
// the CTRL_expressions_ prefix). Pose "neutral" (all controls 0) is always evaluated first.
// eval writes, per pose, RigLogic's joint outputs and blend shape channel weights.

#include <dna/BinaryStreamReader.h>
#include <dna/Reader.h>
#include <riglogic/RigLogic.h>
#include <status/Status.h>

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <map>
#include <sstream>
#include <string>
#include <vector>

namespace {

struct Blob {
    std::ofstream bin;
    std::ostringstream index;
    std::uint64_t offset = 0;
    bool first = true;

    template <typename T>
    void array(const std::string& name, const char* dtype, const T* data, std::size_t count, std::size_t cols) {
        bin.write(reinterpret_cast<const char*>(data), static_cast<std::streamsize>(count * sizeof(T)));
        index << (first ? "" : ",\n") << "  \"" << name << "\": {\"dtype\": \"" << dtype << "\", \"offset\": " << offset
              << ", \"count\": " << count << ", \"cols\": " << cols << "}";
        first = false;
        offset += count * sizeof(T);
    }
    void f32(const std::string& n, const std::vector<float>& v, std::size_t cols) { array(n, "f4", v.data(), v.size(), cols); }
    void u32(const std::string& n, const std::vector<std::uint32_t>& v, std::size_t cols) { array(n, "u4", v.data(), v.size(), cols); }
};

std::string lower(std::string s) {
    std::transform(s.begin(), s.end(), s.begin(), [](unsigned char c) { return std::tolower(c); });
    return s;
}

std::string quote(const std::string& s) {
    std::string out = "\"";
    for (char c : s) {
        if (c == '"' || c == '\\') out += '\\';
        out += c;
    }
    return out + "\"";
}

std::string names(const std::vector<std::string>& v) {
    std::string out = "[";
    for (std::size_t i = 0; i < v.size(); ++i) out += (i ? ", " : "") + quote(v[i]);
    return out + "]";
}

struct Dna {
    dna::FileStream* stream = nullptr;
    dna::BinaryStreamReader* reader = nullptr;
    explicit Dna(const char* path) {
        stream = dna::FileStream::create(path, dna::FileStream::AccessMode::Read, dna::FileStream::OpenMode::Binary);
        reader = dna::BinaryStreamReader::create(stream, dna::DataLayer::All);
        reader->read();
        if (!dna::Status::isOk()) {
            std::fprintf(stderr, "dna read failed: %s\n", dna::Status::get().message);
            std::exit(1);
        }
    }
    ~Dna() {
        dna::BinaryStreamReader::destroy(reader);
        dna::FileStream::destroy(stream);
    }
};

void interleave3(av::ConstArrayView<float> x, av::ConstArrayView<float> y, av::ConstArrayView<float> z, std::vector<float>& out) {
    out.resize(x.size() * 3);
    for (std::size_t i = 0; i < x.size(); ++i) {
        out[i * 3] = x[i];
        out[i * 3 + 1] = y[i];
        out[i * 3 + 2] = z[i];
    }
}

int dump(const char* path, const std::string& out) {
    Dna d(path);
    const dna::Reader* r = d.reader;
    Blob b;
    b.bin.open(out + ".bin", std::ios::binary);

    std::vector<std::string> jointNames, controls, channels, meshNames;
    std::vector<std::uint32_t> parents;
    for (std::uint16_t j = 0; j < r->getJointCount(); ++j) {
        jointNames.emplace_back(r->getJointName(j).c_str());
        parents.push_back(r->getJointParentIndex(j));
    }
    for (std::uint16_t c = 0; c < r->getRawControlCount(); ++c) controls.emplace_back(r->getRawControlName(c).c_str());
    for (std::uint16_t c = 0; c < r->getBlendShapeChannelCount(); ++c) channels.emplace_back(r->getBlendShapeChannelName(c).c_str());

    std::vector<float> v;
    b.u32("joint_parents", parents, 1);
    interleave3(r->getNeutralJointTranslationXs(), r->getNeutralJointTranslationYs(), r->getNeutralJointTranslationZs(), v);
    b.f32("joint_translations", v, 3);
    interleave3(r->getNeutralJointRotationXs(), r->getNeutralJointRotationYs(), r->getNeutralJointRotationZs(), v);
    b.f32("joint_rotations", v, 3);

    auto lod0 = r->getMeshIndicesForLOD(0);
    for (std::uint16_t m : lod0) {
        const std::string mn = r->getMeshName(m).c_str();
        meshNames.push_back(mn);
        interleave3(r->getVertexPositionXs(m), r->getVertexPositionYs(m), r->getVertexPositionZs(m), v);
        b.f32(mn + "/positions", v, 3);
        auto us = r->getVertexTextureCoordinateUs(m);
        auto vs = r->getVertexTextureCoordinateVs(m);
        v.resize(us.size() * 2);
        for (std::size_t i = 0; i < us.size(); ++i) {
            v[i * 2] = us[i];
            v[i * 2 + 1] = vs[i];
        }
        b.f32(mn + "/uvs", v, 2);
        interleave3(r->getVertexNormalXs(m), r->getVertexNormalYs(m), r->getVertexNormalZs(m), v);
        b.f32(mn + "/normals", v, 3);

        auto lp = r->getVertexLayoutPositionIndices(m);
        auto lt = r->getVertexLayoutTextureCoordinateIndices(m);
        auto ln = r->getVertexLayoutNormalIndices(m);
        std::vector<std::uint32_t> layouts(lp.size() * 3);
        for (std::size_t i = 0; i < lp.size(); ++i) {
            layouts[i * 3] = lp[i];
            layouts[i * 3 + 1] = lt[i];
            layouts[i * 3 + 2] = ln[i];
        }
        b.u32(mn + "/layouts", layouts, 3);

        std::vector<std::uint32_t> faceIdx, faceSizes;
        for (std::uint32_t f = 0; f < r->getFaceCount(m); ++f) {
            auto fl = r->getFaceVertexLayoutIndices(m, f);
            faceSizes.push_back(static_cast<std::uint32_t>(fl.size()));
            faceIdx.insert(faceIdx.end(), fl.begin(), fl.end());
        }
        b.u32(mn + "/face_layouts", faceIdx, 1);
        b.u32(mn + "/face_sizes", faceSizes, 1);

        std::vector<std::uint32_t> skinCounts, skinJoints;
        std::vector<float> skinWeights;
        for (std::uint32_t p = 0; p < r->getSkinWeightsCount(m); ++p) {
            auto w = r->getSkinWeightsValues(m, p);
            auto j = r->getSkinWeightsJointIndices(m, p);
            skinCounts.push_back(static_cast<std::uint32_t>(w.size()));
            skinWeights.insert(skinWeights.end(), w.begin(), w.end());
            skinJoints.insert(skinJoints.end(), j.begin(), j.end());
        }
        b.u32(mn + "/skin_counts", skinCounts, 1);
        b.u32(mn + "/skin_joints", skinJoints, 1);
        b.f32(mn + "/skin_weights", skinWeights, 1);

        std::vector<std::uint32_t> bsChannel, bsCounts, bsVerts;
        std::vector<float> bsDeltas;
        for (std::uint16_t t = 0; t < r->getBlendShapeTargetCount(m); ++t) {
            bsChannel.push_back(r->getBlendShapeChannelIndex(m, t));
            auto vi = r->getBlendShapeTargetVertexIndices(m, t);
            bsCounts.push_back(static_cast<std::uint32_t>(vi.size()));
            bsVerts.insert(bsVerts.end(), vi.begin(), vi.end());
            interleave3(r->getBlendShapeTargetDeltaXs(m, t), r->getBlendShapeTargetDeltaYs(m, t), r->getBlendShapeTargetDeltaZs(m, t), v);
            bsDeltas.insert(bsDeltas.end(), v.begin(), v.end());
        }
        b.u32(mn + "/bs_channel", bsChannel, 1);
        b.u32(mn + "/bs_counts", bsCounts, 1);
        b.u32(mn + "/bs_vertices", bsVerts, 1);
        b.f32(mn + "/bs_deltas", bsDeltas, 3);
    }

    std::ofstream js(out + ".json");
    js << "{\n\"name\": " << quote(r->getName().c_str()) << ",\n\"lods\": " << r->getLODCount()
       << ",\n\"coordinate_system\": [" << int(r->getCoordinateSystem().xAxis) << ", " << int(r->getCoordinateSystem().yAxis) << ", "
       << int(r->getCoordinateSystem().zAxis) << "],\n\"translation_unit\": " << int(r->getTranslationUnit())
       << ",\n\"rotation_unit\": " << int(r->getRotationUnit()) << ",\n\"joints\": " << names(jointNames)
       << ",\n\"raw_controls\": " << names(controls) << ",\n\"blend_shape_channels\": " << names(channels)
       << ",\n\"meshes\": " << names(meshNames) << ",\n\"arrays\": {\n" << b.index.str() << "\n}\n}\n";
    std::printf("%s: %zu joints, %zu controls, %zu channels, %zu LOD0 meshes, %.1f MB\n", path, jointNames.size(), controls.size(),
                channels.size(), meshNames.size(), double(b.offset) / 1e6);
    return 0;
}

int eval(const char* path, const char* posesPath, const std::string& out) {
    Dna d(path);
    const dna::Reader* r = d.reader;
    std::map<std::string, std::uint16_t> byName;
    for (std::uint16_t c = 0; c < r->getRawControlCount(); ++c) {
        std::string n = lower(r->getRawControlName(c).c_str());
        for (const std::string prefix : {"ctrl_expressions.", "ctrl_expressions_"}) {
            if (n.rfind(prefix, 0) == 0) n = n.substr(prefix.size());
        }
        byName[n] = c;
    }

    std::vector<std::string> poseNames{"neutral"};
    std::map<std::string, std::vector<std::pair<std::uint16_t, float>>> poses;
    std::ifstream in(posesPath);
    std::string pose, control;
    float value;
    while (in >> pose >> control >> value) {
        auto it = byName.find(lower(control));
        if (it == byName.end()) {
            std::fprintf(stderr, "unknown control %s (pose %s)\n", control.c_str(), pose.c_str());
            return 1;
        }
        if (!poses.count(pose)) poseNames.push_back(pose);
        poses[pose].push_back({it->second, value});
    }

    // UE's FRigLogicConfiguration defaults: quaternion outputs (x, y, z, w), Euler data read as ZYX.
    rl4::Configuration config;
    config.calculationType = rl4::CalculationType::AnyVector;
    config.rotationType = rl4::RotationType::Quaternions;
    config.rotationOrder = rl4::RotationOrder::ZYX;
    rl4::RigLogic* rig = rl4::RigLogic::create(r, config);
    rl4::RigInstance* inst = rl4::RigInstance::create(rig);
    inst->setLOD(0);

    Blob b;
    b.bin.open(out + ".bin", std::ios::binary);
    std::vector<float> joints, shapes;
    std::size_t jointCols = 0, shapeCols = 0;
    for (const auto& name : poseNames) {
        std::vector<float> raw(inst->getRawControlCount(), 0.0f);
        for (auto [c, v] : poses[name]) raw[c] = v;
        inst->setRawControlValues(raw.data());
        rig->calculate(inst);
        auto jo = inst->getJointOutputs();
        auto bo = inst->getBlendShapeOutputs();
        jointCols = jo.size();
        shapeCols = bo.size();
        joints.insert(joints.end(), jo.begin(), jo.end());
        shapes.insert(shapes.end(), bo.begin(), bo.end());
    }
    auto neutral = rig->getNeutralJointValues();
    std::vector<float> nv(neutral.begin(), neutral.end());
    b.f32("joint_outputs", joints, jointCols);
    b.f32("blend_shape_outputs", shapes, shapeCols);
    b.f32("neutral_joint_values", nv, nv.size());

    std::ofstream js(out + ".json");
    js << "{\n\"poses\": " << names(poseNames) << ",\n\"joint_count\": " << r->getJointCount()
       << ",\n\"rotation\": \"quaternion_xyzw\",\n\"arrays\": {\n" << b.index.str() << "\n}\n}\n";
    std::printf("%zu poses, %zu joint values, %zu blend shape outputs per pose\n", poseNames.size(), jointCols, shapeCols);
    rl4::RigInstance::destroy(inst);
    rl4::RigLogic::destroy(rig);
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc >= 4 && std::strcmp(argv[1], "dump") == 0) return dump(argv[2], argv[3]);
    if (argc >= 5 && std::strcmp(argv[1], "eval") == 0) return eval(argv[2], argv[3], argv[4]);
    std::fprintf(stderr, "usage: mhdna dump <dna> <out> | mhdna eval <dna> <poses.txt> <out>\n");
    return 2;
}
